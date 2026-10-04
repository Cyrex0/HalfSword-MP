# Shared-memory IPC (HSMP-SHM)

How the game's Lua mods and `hsmp-sidecar.exe` exchange data. This page describes the
implementation: ABI 2, `crates/hsmp-ipc`, the game-side native module `crates/hsmp-native`, the
Lua facade `mods/shared/hsmp_ipc.lua` and the sidecar's `server/src/sidecar/ipc_shm.rs`.

Related: [architecture.md](architecture.md) (where this sits), [protocol.md](protocol.md) (the
wire the same records travel on), [lua-mods.md](lua-mods.md) (writing mod code against the
facade), [ue4ss.md](ue4ss.md) (the UE4SS build the native module is pinned to).

## 1. Overview

| Decision | What |
|---|---|
| Transport | One named, pagefile-backed shared-memory segment per game process (`CreateFileMappingW` / `MapViewOfFile`). The game creates it; the sidecar and the tools only open it. |
| Game side | `HSMPNative`, a UE4SS C++ mod (`Mods/HSMPNative/dlls/main.dll`) that links the Rust staticlib `hsmp-native`. All shared-memory access and all Lua marshalling are in Rust. The C++ layer registers the module and builds the engine-reflection table for native sampling and servo. `hsmp_lua.dll`, built from the same crate, is a fallback loaded through `package.loadlib`. |
| Primitives | Seqlock latest-value slots for small data, lock-free triple buffers for large blobs, SPSC rings for messages. Every primitive has one writer thread in one process. |
| One schema | `crates/hsmp-ipc/src/schema/<domain>.rs` defines every shared struct as `#[repr(C)]` plain-old-data. `hsmp-tools gen-ipc` generates the C header and the Lua schema from it. A layout hash in the segment header catches any mismatch. |
| One representation | A record is written once by the game, copied unchanged onto the wire by the sidecar behind an 8-byte header, and validated in place by the server. No JSON, codec or re-encoding on any hop. |
| Reliability | Per-side epochs, heartbeats, a world epoch per level change, a session epoch per server session, and process handles for death. Events are hints; state is truth. |
| Separate sidecar | Networking, crypto and the career-save guard stay out of the game process. A crash on either side does not take the other down, and the game process gets no sockets, threads or crypto from HSMP. |

```
 Game process                                              Sidecar process (hsmp-sidecar.exe)
 ┌───────────────────────────────────────────────┐        ┌──────────────────────────────────────┐
 │ Lua mods (one Lua state each, game thread)    │        │ tokio runtime: hsmp-net, session,    │
 │   shared/hsmp_ipc.lua (facade)                │        │ combat / world / loadout clients     │
 │        │ HSMPNative.* calls                   │        │        │                             │
 │ HSMPNative (main.dll -> Rust hsmp-native)     │        │ thread hsmp-ipc: owns the segment    │
 │   game-thread funnel, event log, bus,         │        │ thread hsmp-poseplay: peer_play only │
 │   native sampling / servo                     │        │                                      │
 └───────────────┬───────────────────────────────┘        └───────────────┬──────────────────────┘
                 ▼                                                        ▼
   Local\HSMP.ipc.2.<gamePID>.<gameCreateTimeHex>   (3.57 MiB, owner-only DACL)
   header │ game slots │ game blobs │ peer table │ peer loadouts │ state │ G2S │ S2G │ bus │ DevCtl
   doorbells: <name>.g2s, <name>.s2g (auto-reset events)
```

## 2. Segment layout

The whole segment is one `#[repr(C)] struct Segment` (`crates/hsmp-ipc/src/segment.rs`), so
every offset is a compile-time constant. Each region starts on a 4 KiB boundary. The table is
for the current layout (`SEGMENT_SIZE = 0x39B000`, 3,780,608 bytes); the generated header
`crates/hsmp-native/cpp/gen/hsmp_ipc.h` has every offset.

| Region | Offset | Size | Writer | Contents |
|---|---|---|---|---|
| `header` | `0x000000` | 4 KiB | both (fields owned per side) | Frozen prefix, one block per side, shared epochs (§2.1) |
| `game_out` | `0x001000` | 4 KiB | game | Seqlock slots: `local_root`, `local_weapon`, `local_pose`, `pose_lead`, `local_vitals`, `kit`, `kit_rules_req`, `body` |
| `game_blobs` | `0x002000` | 364 KiB | game | Triple buffers: `world_out`, `loadout`, `world_manifest_out`, `world_dyn_out`, `world_hash` |
| `peers` | `0x05D000` | 132 KiB | sidecar | `dir` (peer id to slot), `kit_rules`, and `MAX_PEER_SLOTS = 32` × { `play`, `root`, `vitals`, `kit`, `body` } |
| `peer_loadouts` | `0x07E000` | 1,364 KiB | sidecar | 32 triple-buffered `loadout` records |
| `state` | `0x1D3000` | 744 KiB | sidecar | Slots `session`, `link`, `admin`, `world_consistency`; triple buffers `world_remote`, `world_owners`, `world_manifest`, `world_dyn` |
| `g2s` | `0x28D000` | 260 KiB | game | Ring, 512 × 512 B (game to sidecar) |
| `s2g` | `0x2CE000` | 516 KiB | sidecar | Ring, 1024 × 512 B (sidecar to game) |
| `bus` | `0x34F000` | 268 KiB | game | Bus directory and 64 keys × 4 KiB (§6) |
| `devctl` | `0x392000` | 36 KiB | tools | Ring, 64 × 512 B, developer commands (§8) |

Peer slots are indexed by slot, not by peer id: peer ids are u32 and not dense. The sidecar
owns the `dir` mapping and bumps a per-slot generation when it reassigns a slot; the game drops
anything whose `(slot, gen)` changed. 32 slots is above the current 8-player limit on purpose.

### 2.1 Header (`header.rs`)

```rust
#[repr(align(64))]
pub struct HeaderPrefix {        // 128 bytes, identical in every ABI version
    pub magic: [u8; 8],          // b"HSMPSHM\0"
    pub abi_major: u16,          // 2
    pub abi_minor: u16,          // 0
    pub prefix_size: u16,        // 128
    pub header_size: u16,
    pub segment_size: u64,
    pub layout_hash: u64,
    pub init_state: AtomicU32,   // 0 zero pages, 1 initialising, 2 ready, 3 poisoned
    pub refuse_code: AtomicU32,
    pub refuse_detail: [AtomicU64; 8],  // ASCII, NUL-padded
    pub _pad: [u8; 24],
}

#[repr(align(64))]
pub struct SideBlock {           // one per side, written only by that side
    pub pid: AtomicU32,
    pub state: AtomicU32,        // Absent / Starting / Ready / Loading (game) / Closing
    pub epoch: AtomicU64,        // random per process start; 0 = never attached
    pub create_time: AtomicU64,  // FILETIME of the process (PID-reuse guard)
    pub caps: AtomicU64,         // capabilities this side offers
    pub hb_count: AtomicU64,     // +1 per game frame / per sidecar loop
    pub hb_qpc: AtomicU64,       // QueryPerformanceCounter at the last beat
    pub build_id: [AtomicU64; 4],
    pub counters: [AtomicU64; 16],
}

#[repr(align(4096))]
pub struct Header {
    pub prefix: HeaderPrefix,
    pub game: SideBlock,
    pub sidecar: SideBlock,
    pub caps_effective: AtomicU64,  // sidecar at attach: game.caps & sidecar.caps
    pub qpc_freq: AtomicU64,        // game at init
    pub world_epoch: AtomicU32,     // game: +1 at every world leave
    pub game_lua_gen: AtomicU32,    // game: +1 on UE4SS "restart all mods"
    pub session_epoch: AtomicU32,   // sidecar: +1 on every new server session
    pub resync_req: AtomicU32,      // sidecar: +1 when S2G overflowed
    pub attach_count: AtomicU32,    // sidecar: +1 on every attach
    pub _r: u32,
}
```

The 16 counters per side are diagnostics only: `torn_retry`, `busy`, `ring_full`, `overflow`,
`resync`, `stale_epoch`, `bad_record`, `wrong_thread`, `decode_err`, `too_big`, `msgs_out`,
`msgs_in`, `slot_writes`, `slot_reads`, `ring_high_water`, `panics`.

## 3. Primitives

All cross-process data is accessed through atomics. Payload words are copied with `Relaxed`
`AtomicU64` loads and stores (plain `mov`s on x86-64), so the Rust memory model has no data race
even while a reader races a writer. Readers clamp every count and length to the compile-time
capacity, so a scribbled segment gives wrong data, never an out-of-bounds read.

### 3.1 Seqlock slot (`seqlock.rs`)

```text
writer (single):                        reader (any number, never blocks the writer):
  s = start_seq(seq)       // odd        for 4 attempts:
  seq.store(s, Relaxed)                     s1 = seq.load(Acquire)
  fence(Release)                            s1 == 0: return Empty
  copy payload words                        s1 odd:  spin, retry
  fence(Release)                            copy payload words; fence(Acquire)
  seq.store(s + 1, Release) // even         seq == s1: return Ok(copy, s1)
                                          return Busy   // keep the last good value
```

- Every slot payload starts with a 32-byte `SlotMeta { writer_epoch: u64, world_epoch: u32,
  session_epoch: u32, valid: u32, sample_seq: u32, t_us: u64 }`. Readers check `writer_epoch`
  against the writing side's current epoch, so data from a previous process is never used.
- `seq` doubles as a change counter: comparing it with the last consumed value is one load.
- A writer that dies mid-write leaves `seq` odd; readers then get `Busy` until the restarted
  writer publishes again (its next write starts from the odd value and ends on the next even one).

### 3.2 Triple buffer (`triple.rs`)

Large blobs (world state, loadouts, manifests) use a lock-free triple buffer, so a fast writer
can never starve a reader of a 100 KiB blob.

- Three buffers and a control word `middle`: bits 0–1 the buffer index, bit 2 `DIRTY`, bits
  8–31 a generation.
- The writer fills its back buffer, then swaps it into `middle` with `DIRTY` and the next
  generation, keeping the old middle as its new back buffer.
- The reader, if `DIRTY` is set, swaps its front buffer into `middle` and takes the old middle.
- The writer's back index and the reader's front index are stored in the segment (each written
  only by its owner), so a restarted process resumes without a reset handshake. The game
  initialises every triple buffer, including the sidecar-written ones, at creation.

### 3.3 SPSC ring (`ring.rs`)

- Power-of-two slots of 512 bytes: a 40-byte `RecordHeader { len: u16, kind: u16, aux: u16,
  flags: u16, peer: u32, _r: u32, producer_epoch: u64, seq: u64, req_id: u64 }` and up to
  472 bytes of payload. `kind`, `aux` and `peer` are the fields of the wire header (§4.4).
- `tail` is written only by the producer, `head` only by the consumer. `seq` is the record's
  ring position, so the consumer catches a gap, duplicate or scribble in one compare.
- **Restart.** A restarted producer continues from `tail` with its new epoch, and the consumer
  drops records whose `producer_epoch` is not the producer's current epoch. A restarted consumer
  sets `head = tail`: everything pending was addressed to its predecessor.
- A payload over 472 bytes is refused with `TooBig`. Anything larger is state and goes into a
  blob, so there is no fragmentation code.

### 3.4 Game-thread funnel

Every mod has its own Lua state, and each can call `send` or `bus_put`. All of them run on the
game thread, so the G2S ring stays single-producer across processes:

- `hsmp-native` records the game-thread id at the first `frame()` call. Any IPC entry point
  called from another thread returns `nil, "wrong thread"` and counts it.
- The S2G ring has one consumer, the native module. `frame()` drains it into an in-process event
  log of 4096 records; each Lua state reads the log through its own cursor (§5.3).

### 3.5 Doorbells

- **G2S** (`<name>.g2s`, auto-reset event). `frame()` calls `SetEvent` at most once per frame,
  and only if something was written since the last frame. This is the only syscall on the game's
  hot path.
- The sidecar's `hsmp-ipc` thread waits on it with a 1 ms timeout, so a new sample reaches the
  network within about 1 ms even without the doorbell.
- **S2G** (`<name>.s2g`) exists for tools. The game never waits; it polls once per frame.

## 4. Schema, code generation and versioning

### 4.1 Records

Every replicated datum is a record: a `#[repr(C)]` struct declared once with `ipc_pod!` in
`crates/hsmp-ipc/src/schema/<domain>.rs` and bound to a kind with `record!`.

- **Field types:** `u8`..`u64`, `i8`..`i64`, `f32`, `f64`, arrays, nested `ipc_pod!` structs,
  `layout::Str<N>` (UTF-8, NUL-padded; a Lua string) and `layout::Bool` (0/1; a Lua boolean).
  Explicit padding fields are named `_...`; sizes are multiples of 8. No pointers, `Vec`,
  `String` or `Option`. Enumerations are integer fields with a code table in the domain's
  `ENUMS` (`S.ENUMS.<table>.<NAME>` in Lua).
- **Fixed** records are one struct. **Variable** records are a head struct with a row count
  plus up to `max` rows of one row struct. In shared memory a variable record sits in a
  `VarBuf<Head, CAP>`; its first `payload_len()` bytes are the wire and ring payload. In Lua
  the rows are `t.rows`.
- **Validation** (`record::view`) is the only way untrusted bytes become a record: exact size,
  row count within `max`, every float finite, every `Str` canonical UTF-8, every `Bool` 0 or 1,
  then the record's own `check` (enum ranges, world bounds). It is zero-copy when the input is
  aligned. There is no `unsafe` in the parsers.
- **Quantisation** happens once, at the source: pose codec v2 frames, smallest-three world
  quaternions, u16 vitals. Nobody re-encodes them.

### 4.2 Kind ids, domains and capability bits

A kind id is `domain << 8 | n`. Kind 0 is never valid. A unit test enforces unique ids.

| Domain | High byte | File |
|---|---|---|
| pose | `0x01` | `schema/pose.rs` |
| session | `0x02` | `schema/session.rs` |
| combat | `0x03` | `schema/combat.rs` |
| world | `0x04` | `schema/world.rs` |
| loadout | `0x05` | `schema/loadout.rs` |
| interact | `0x06` | `schema/interact.rs` |
| bus | `0x07` | `schema/bus.rs` |
| dev | `0x08` | `schema/dev.rs` |

`RecordInfo` (each domain's `RECORDS`) gives a kind's name, layout, capability bit, `flow`
(where it may appear: `C2S`, `S2C`, `G2S`, `S2G`, `LOCAL`), `chan` (the hsmp-net channel:
`Latest(stream)`, `RelLatest(stream)`, `Reliable`, `Ordered`, `None`) and its validator.
Several kinds may share one layout.

`SlotInfo` (each domain's `SLOTS`) names a record slot: its record kind, its form (`Slot`,
`Blob`, `PeerSlot`, `PeerBlob`, `Bus`), its writer, its capability bit and whether it is
world-scoped. `Segment::slot_ref(name, peer)` resolves a slot by name, so the native module,
the sidecar and the tools need no per-slot code. Three typed slots of the pose domain are not
records and never go on the wire: `pose_lead`, `peer_dir` and `peer_play` (evaluated playback).

Capability bits (u64): `POSE` 0x1, `STATE` 0x2, `VITALS` 0x4, `LOADOUT_KIT` 0x8, `WORLD` 0x10,
`QUEUES` 0x20, `INTERACT` 0x40, `COMBAT` 0x80, `BUS` 0x100, `DEVCTL` 0x200, `NATIVE_SAMPLE`
0x400, `NATIVE_SERVO` 0x800, `POSEPLAY_IN_GAME` 0x1000 (defined, not implemented).

- The game offers everything from `POSE` to `DEVCTL`. It adds `NATIVE_SAMPLE` and
  `NATIVE_SERVO` only when the C++ mod registered the engine-reflection table (main.dll on the
  pinned UE4SS build).
- `caps_effective = game.caps & sidecar.caps`, written by the sidecar at attach.
- `HSMP_IPC_GAME_CAPS=<hex>` masks what the game offers, for A/B runs.

### 4.3 Code generation

`hsmp-tools gen-ipc` writes two files from the layout descriptors (`crates/hsmp-ipc/src/gen.rs`):

| File | Contents |
|---|---|
| `crates/hsmp-native/cpp/gen/hsmp_ipc.h` | C structs, `static_assert` on the size and offset of every field, `HSMP_OFF_*` offsets, kind ids, cap bits, flags, refuse codes, `HSMP_IPC_LAYOUT_HASH` |
| `mods/shared/hsmp_ipc_schema.lua` | `ABI_MAJOR`, `LAYOUT_HASH`, kinds, slots, enums and caps for the facade and the offline Lua tests |

Generated files are never edited by hand. `hsmp-tools gen-ipc --check` diffs them. The G0 check
`ipc_schema` runs that diff and compiles the header with `cl /W4 /WX` as C11 and C++17 when MSVC
is installed.

### 4.4 Wire framing (protocol v6)

A channel message is `[WireHdr { kind: u16, aux: u16, peer: u32 }][record payload]`
(`crates/hsmp-ipc/src/wire.rs`).

- Client to server: `peer = 0`; the connection identifies the sender.
- Server to client: `peer` is the player the record is about or from. The server relays by
  copying the message and patching `peer` (and `aux`, e.g. the pose relay interval) in place.
- Ring records carry the same three fields, so a message moves between the network and a ring
  as a copy.

The transport itself (handshake, encryption, channels, fragmentation) is in
[protocol.md](protocol.md).

### 4.5 Layout hash and refusal

`layout_hash` is FNV-1a 64 over the canonical description of the whole `Segment` (every struct
name, field name, type, offset and size, in order) plus `ABI_MAJOR`. It is a `const fn`, so both
binaries carry it as a constant. A rename, reorder, resize or retype anywhere changes it, even
when the total size stays the same. `COMPAT_HASHES` (older hashes a build still accepts) is
empty.

The sidecar refuses a segment it cannot use. It writes the code and a detail string into the
header (except for `BadMagic`, so it never writes into foreign memory) and exits with
`69 + code`:

| Code | Name | Exit | When |
|---|---|---|---|
| 1 | `BadMagic` | 70 | Not an HSMP segment |
| 2 | `AbiMismatch` | 71 | ABI major or layout hash differs |
| 3 | `WrongParent` | 72 | `game.pid` or `game.create_time` does not match `--parent-pid`, or `--parent-pid` is missing |
| 4 | `NotReady` | 73 | The mapping did not open, or the game did not reach `init_state = ready`, within 2 s |
| 5 | `OwnerMismatch` | 74 | The mapping's owner SID is not the current user |
| 6 | `SizeMismatch` | 75 | The mapping is smaller than the header claims |
| 7 | `Poisoned` | 76 | A caught native panic disabled IPC in the game |
| 8 | `Busy` | 77 | Reserved for "another sidecar is attached"; not raised at present |

A sidecar started without `--ipc shm:<name>` exits with 64.

On the game side, the facade compares `HSMPNative.abi()` with `hsmp_ipc_schema.lua` at load.
On a mismatch, or with no native module, IPC stays unavailable: every call returns nothing,
nothing falls back to files, and HSMPMenu shows one of two messages: "Mod and helper program
versions do not match - reinstall HSMP" or "Helper program did not start - reinstall HSMP".

## 5. Game side

### 5.1 The native module

| Build | Artefact | Loaded by |
|---|---|---|
| main (default) | `ue4ss/Mods/HSMPNative/dlls/main.dll` | UE4SS as a C++ mod (`HSMPNative : 1` in `mods.txt`). On `on_lua_start` it registers the global `HSMPNative` in every `HSMP*` Lua state before that mod's `main.lua` runs. |
| fallback | `ue4ss/Mods/HSMPNative/dlls/hsmp_lua.dll` | The facade, through `package.loadlib(..., "luaopen_hsmp_lua")`. If main.dll already registered `HSMPNative` in that state, it returns that table. |

Rules the binding follows:

- **Never raise.** No `luaL_error`. Every failure returns `nil, "<reason>"`. Every FFI entry
  point is wrapped in `catch_unwind`; a caught panic poisons the segment (`init_state = 3`) and
  every later call returns `nil, "disabled"`.
- **Game thread only** (§3.4).
- **Process-global state.** The mapping survives UE4SS "restart all mods"; `game_lua_gen` is
  bumped and the new Lua states re-register their cursors.
- **One copy per process.** A second copy of the DLL loaded from another path refuses
  `ipc_open` with `"ipc owned by another native copy in this process"`, so there is never a
  second G2S producer.
- **Pinned UE4SS.** main.dll refuses to register on a UE4SS.dll whose build is not the pinned
  one (see [ue4ss.md](ue4ss.md)). `HSMPNATIVE_ALLOW_UNPINNED=1` exists for the offline harness
  only.
- It writes `HSMPNative.log` next to the `Mods` folder.

Build instructions: `crates/hsmp-native/README.md`.

### 5.2 Native API (`HSMPNative.*`)

Kinds and slots are passed by name (`hsmp_ipc_schema.lua`). Tables passed as `out` belong to
the caller and are refilled in place, so a steady-state frame allocates nothing.

| Call | Returns | Notes |
|---|---|---|
| `ipc_open()` | `name` \| `nil, err` | Creates the mapping and both doorbells, or returns the existing name. |
| `ipc_info()` | table | `name, state, abi_major, abi_minor, layout_hash, caps_game, caps_sidecar, caps_effective, game_epoch, sidecar_epoch, sidecar_pid, sidecar_state, sidecar_hb_age_s, world_epoch, session_epoch, attach_count, game_lua_gen, refuse_code, refuse_detail, counters` |
| `frame(world_key)` | `flags` | One pump per frame, called by HSMPSync only (§5.4). |
| `flags()` | `flags` | The flags of the last `frame()`, for the other mods. |
| `world_leaving()` / `world_ready(world_key)` | `true` | §7.3 |
| `put_root`, `put_weapon`, `put_pose`, `put_lead` | `true` \| `nil, err` | Hot paths with scalar and flat-array arguments. The module builds the record once (rotator to quaternion, pose codec v2). |
| `put(slot, t)` | `true` \| `nil, err` | A game-written slot or blob. `err`: `bad:<field>`, `bad:<reason>`, `too_big`, `cap`, `not open`. |
| `get(slot, last, out?)` | `ver, t` \| `ver` \| `nil` | Any slot. Returns only `ver` when unchanged since `last`, `nil` when never written. |
| `peers(out)` | `n` | `out[i] = {id, slot, gen, nick, ...}` from the peer directory. |
| `peer_play(slot, out, last)` | `seq` \| `nil` | A peer's evaluated playback sample. |
| `peer(slot, peer_slot, last, out?)` | `ver, t` \| `nil` | Per-peer record slots. |
| `send(kind, t)` | `req_id` \| `nil, err` | G2S ring. `err` adds `full`, `wrong thread`, `disabled`. |
| `subscribe(kinds)` | `true` | Optional filter of S2G kinds for this Lua state. |
| `poll(max, out)` | `n` | `out[i] = {kind, peer, aux, req_id, data}`. A cursor that fell behind gets one `{kind = "resync"}` entry. |
| `bus_put(key, t)` / `bus_get(key, last, out?)` | as `put` / `get` | §6 |
| `dev_poll(max, out)` | `n` | `out[i] = {kind = "dev_cmd", data = t}`, each command once per Lua state. |
| `spawn`, `spawn_capture`, `capture_poll`, `proc_alive`, `proc_exit_code`, `proc_kill`, `current_pid` | | §5.5 |
| `sample_config`, `sample_local`, `sample_status` | | §7.1 |
| `servo_config`, `servo_bodies`, `servo_weapon`, `servo_status`, `neutralise_config`, `neutralise`, `neutralise_status` | | §7.2 |
| `abi()` | `major, minor, layout_hash_hex` | |
| `now_us()`, `thread_ok()` | number, bool | |

**Table rules.** Field names are the Rust names; `_` padding never appears. Integers are Lua
integers (a float truncates toward zero and saturates; an integer wraps to the field width;
`true` is 1). Floats must be finite, also after rounding to `f32`. Arrays are 1-based (extra
elements ignored, missing ones default). A `Str` is truncated on a UTF-8 boundary and must be
valid UTF-8 without NUL. A missing field is 0, `""` or `false`. A variable record's count is
`#t.rows`; more than `max` is `too_big`. After marshalling, the record's own check runs, so the
game never writes a record that the sidecar or server would refuse.

These rules are implemented twice, in `crates/hsmp-native/src/marshal.rs` and in the Lua mock
`tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua`, and
`lua-tests/lib/records_conformance.lua` checks one against the other.

### 5.3 Frame flags and events

`frame()` returns a bitmask:

| Flag | Value | Meaning |
|---|---|---|
| `SESSION_CHANGED` | 0x1 | The `session` slot changed |
| `EVENTS` | 0x2 | New S2G records reached the event log |
| `PEERS_CHANGED` | 0x4 | The peer directory changed |
| `SIDECAR_RESET` | 0x8 | A sidecar attached or a new one replaced it (new epoch) |
| `WORLD_CHANGED` | 0x10 | The world key changed |
| `RESYNC` | 0x20 | The sidecar overflowed S2G, or this state's cursor fell behind; re-read state |
| `REFUSED` | 0x40 | A refuse code is set in the header |
| `SIDECAR_LOST` | 0x80 | The sidecar process exited |
| `SIDECAR_STALLED` | 0x100 | The sidecar is attached but its heartbeat is older than 1 s |
| `STATE_CHANGED` | 0x200 | A world state blob changed |

The S2G ring has one consumer in the game, so the native module fans events out. `frame()`
drains the ring into a 4096-record in-process log. Each Lua state has a cursor, optionally
filtered with `subscribe`. A cursor that falls more than 4096 records behind gets one `resync`
entry and jumps to the oldest record. Records are drained only while a live sidecar is attached;
records left over from a dead sidecar are dropped as stale when the next one attaches.

### 5.4 The facade (`mods/shared/hsmp_ipc.lua`)

`build-and-deploy.ps1` copies `mods/shared/*.lua` into every mod, so each mod's Lua state has
its own facade instance.

```lua
local IPC = require("hsmp_ipc")
IPC.init{ mod = "HSMPAvatars", state_dir = STATE_DIR, log = Log }
local seq = IPC.peer_play(slot, out, last)
local s = IPC.rec("session")          -- cached decoded table, one decode per change
IPC.send("game_status", t)            -- G2S, with a retry queue
for _, e in ipairs(IPC.events("damage_in")) do ... end
```

- **Binding:** the `HSMPNative` global, else `require "hsmp_lua"`, else `package.loadlib`.
  `IPC.init` checks the ABI and layout hash, then calls `ipc_open` (idempotent).
- **Capabilities:** `IPC.use(kind)` is true when the kind's capability bit is negotiated
  (`caps_effective`; the game's own caps for game-local kinds such as the bus).
- **Record caches:** `IPC.rec(slot)` and `IPC.peer_rec(slot, peer_id)` keep one decoded table
  per slot version per Lua state. `IPC.peer_dir()` caches the peer directory (refreshed on
  `PEERS_CHANGED` or every 0.25 s).
- **Sends:** `IPC.send` returns a `req_id`, or `true` when the message was queued because the
  ring is full or the sidecar is not attached yet. The retry queue holds 256 messages per Lua
  state and is flushed in order. A full ring logs `ipc_backpressure` once per episode.
- **Events:** `IPC.pump()` drains this state's cursor into per-kind queues for the kinds a mod
  asked for; `IPC.events(kind)` returns and clears them. A `resync` entry sets `IPC.resync`.
- **Refused writes** are logged once per kind and reason (`ipc_write_refused`).
- `IPC.read / write / remove / exists` are plain file helpers for a mod's own config and log
  files. No IPC contract is a file.

### 5.5 Processes

HSMPMenu starts the sidecar, the listen server and a local master through the native module,
not through a shell (`crates/hsmp-native/src/proc.rs`):

- `spawn(exe, args, opts)` uses `CreateProcessW` (no window by default) and puts the child in
  its own job object without kill-on-close, so children survive a game crash and follow
  `--parent-pid` instead.
- The module keeps the process handle. `proc_alive`, `proc_exit_code` and `proc_kill` work only
  on processes it spawned; `proc_kill` terminates the child's whole job. Nothing is ever killed
  by name.
- `spawn_capture` gives the child one anonymous pipe for stdout and stderr. `capture_poll` reads
  it without blocking on the game thread. The server browser uses it for `hsmp-query` and
  `curl`.

The sidecar command line is `hsmp-sidecar --server <addr> --state-dir <dir> --nick <nick>
--parent-pid <game pid> --ipc shm:<name>`.

## 6. Game-local bus

Mods are separate Lua states and need to share some state with each other (which peer drives
which stand-in, the Director's state, spawn requests). That state lives in the `bus` region of
the segment. The game writes and reads it on the game thread; the sidecar never touches it.
Because it is in the segment, `ipc-dump` can show it.

- A bus key is a named seqlock slot (`BusDir` holds up to 64 names of up to 32 bytes) whose
  value is one typed record of up to 4096 bytes (`SlotForm::Bus` in a domain's `SLOTS`). The
  slot's `seq` is the key's generation.
- Keys: `puppets`, `playback` (pose); `standin_dead` (combat); `kit_status`, `standin_weapons`
  (loadout); `pose_yield` (interact); `world_held` (world); `director`, `conn_state`,
  `spectate`, `spawn_status`, `spawn_request`, `travel_request`, `travel_ack`, `ui_request`,
  `return_to_lobby`, `fallback_swap` (session).
- **World-scoped keys** are cleared at every `world_leaving()`: `puppets`, `standin_dead`,
  `playback`, `pose_yield`, `world_held`, `spawn_status`. A cleared key reads as the zeroed
  record, which readers treat as absent.
- Facade: `IPC.bus_put`, `IPC.bus_get`, `IPC.bus_table(key)` (cached per generation),
  `IPC.bus_clear(key)`.

## 7. Native sampling and servo

Reflected UE4SS calls from Lua cost several microseconds each. The local pose sample and the
stand-in servo make dozens of them per frame. The native module can do that work through
`ProcessEvent` and property offsets instead. All stages are **on by default**. Lua keeps the
policy (when, which pawn, death and world checks, targets, gains, metrics) and keeps its own
path as the fallback: any refusal means the Lua path does that frame.

### 7.1 Native local sampling (`NATIVE_SAMPLE`, `crates/hsmp-native/src/sample.rs`)

`sample_local(a)` does what HSMPSync's Lua sender does with about 70 reflected calls per sample
(23 × `GetSocketTransform`, the physics velocities of every body, the held weapons, the control
Blueprint variables, the root, the weapon actor). It writes `local_root`, `local_weapon` and
`local_pose` with the same writers as `put_root` / `put_weapon` / `put_pose`, so the records are
byte-identical to the Lua path's.

Switch: settings key `"native_sample"` (true / false), env `HSMP_NATIVE_SAMPLE=1|0` (overrides),
dev tune `native_sample` through `ipc-ctl`. The pose sender's 5 s log line names the sampler in
use.

### 7.2 Native stand-in servo (`NATIVE_SERVO`)

| Stage | Call | What moves to native | Switch (settings / env / tune) |
|---|---|---|---|
| Body servo | `servo_bodies(a, out)` (`servo.rs`) | The per-body loop of HSMPAvatars' servo: `GetSocketTransform`, then `SetPhysicsLinearVelocity` and `SetPhysicsAngularVelocityInDegrees`, about 66 calls per stand-in frame. A port of Lua's `PURE.servo` that matches it bit for bit. Returns the transforms, velocities and correction sizes for Lua's metrics. | `native_servo` / `HSMP_NATIVE_SERVO` |
| Neutralise | `neutralise(a)` (`neutralise.rs`) | About 29 writes to the stand-in Willie's Blueprint variables (exact names, spaces included; Float, Double, Int, Byte and Bool only) and one `SetAllMotorsAngularDriveParams`. | `native_neutralise` / `HSMP_NATIVE_NEUTRALISE` |
| Weapon servo | `servo_weapon(a, out)` (`servo.rs`) | A held weapon's `GetTransform` and the two velocity sets on its root. | `native_wservo` / `HSMP_NATIVE_WSERVO` |

Settings keys take `true` / `false`; env vars `1` / `0` override them; the dev tune knob of the
same name takes `1` / `0`.

### 7.3 Rules for every native UObject access

The engine is reached through a reflection table that the C++ mod builds at start from 23
UE4SS.dll exports (`cpp/src/ue4ss_reflect.cpp`, `src/reflect.rs`). It exists only with main.dll
on the pinned UE4SS build; elsewhere native sampling and servo report `unavailable`.

1. **Game thread only.** Native UObject calls happen only inside a Lua-to-native call. There is
   no native timer or thread.
2. **No raw `UObject*` across calls.** Objects come from Lua on every call as `GetAddress()`
   integers, are proven live by a weak round trip (index and serial number off UE4SS's object
   array), and checked with `IsA` before any `ProcessEvent`. UE4SS's `FWeakObjectPtr` is never
   used: its constructor allocates a serial number for an object that has none, and on this
   build that faults with an uncatchable access violation. Functions and classes without a serial
   are kept as handles pinned to their address and compared, never dereferenced.
3. **World epoch.** `world_leaving()` or a world-key change drops every native cache without
   touching it. Nothing resolves until `world_ready()`.
4. **Verified parameters.** Every UFunction's parameters and the Transform, Vector, Rotator and
   Quat layouts are verified once per world (name, class, size, struct). A Soft* parameter, a
   missing function or any mismatch disables that stage with a reason in `*_status().why`.
5. **Blueprint names with spaces** are looked up exactly (`GetPropertyByNameInChain`).
6. **No SEH swallowing** of engine faults.

## 8. Lifecycle

### 8.1 Naming

- Mapping: `Local\HSMP.ipc.<ABI_MAJOR>.<gamePID>.<gameCreateTimeHex>`, today
  `Local\HSMP.ipc.2.<pid>.<ctime>`. Doorbells append `.g2s` and `.s2g`.
- The PID makes the name unique among live processes; the creation time prevents a collision
  with an orphaned sidecar holding a mapping of a dead process with a reused PID. `Local\`
  keeps it inside the user's logon session.
- `HSMP_INST` and the state dir are not part of the name, so two game instances on one machine
  are separated even with the same state dir.
- There is no discovery: the game passes the name to the sidecar it spawns.

### 8.2 Attach

```text
game (ipc_open)     CreateFileMappingW(pagefile, owner-only DACL, SEGMENT_SIZE, name)
                    write prefix + game SideBlock (random epoch, caps), init the triple buffers,
                    init_state = ready (Release)
game (HSMPMenu)     spawn hsmp-sidecar ... --parent-pid <pid> --ipc shm:<name>
sidecar (attach)    OpenFileMappingW (never creates), retry for up to 2 s
                    owner SID == own user SID, else OwnerMismatch
                    magic, ABI major, layout hash, size, not poisoned, else refuse
                    game.pid == --parent-pid and game.create_time == the parent's, else WrongParent
                    G2S consumer reset (head = tail)
                    write sidecar SideBlock (random epoch, caps), caps_effective, attach_count + 1,
                    state = Ready
game (frame)        sees a new sidecar epoch: SIDECAR_RESET, opens a SYNCHRONIZE handle on its pid
```

### 8.3 Level changes

1. **Before OpenLevel** (HSMPMatch's pre-hook): `IPC.world_leaving()`. `world_epoch` goes up,
   the game state becomes `Loading`, every world-scoped game record is republished with
   `valid = 0`, native caches are dropped untouched, and the world-scoped bus keys are cleared.
2. **World settled** (HSMPSync, once the world key is stable): `IPC.world_ready(key)`. The game
   state goes back to `Ready`; new samples carry the new `world_epoch`.
3. The sidecar accepts a game record only if `valid == 1`, `writer_epoch` is the game's current
   epoch, and, for world-scoped slots, `world_epoch` is the current one. It sends no pose from
   the old world.
4. Peer slots hold network data, not UObjects, so they survive the level change.

### 8.4 Liveness

| Signal | Who reads it | How | Meaning |
|---|---|---|---|
| Game heartbeat | sidecar | `hb_count` / `hb_qpc`, bumped by every `frame()` | Information only. The game thread can stall for many seconds during a level load. |
| Game death | sidecar | Process handle on the parent (`parent.rs`, checked every 500 ms) | Authoritative. The sidecar sends Leave, runs the career-save guard and exits. |
| Sidecar heartbeat | game | `hb_*`, bumped by every `hsmp-ipc` loop (at most 1 ms apart) | Older than 1 s while attached: `SIDECAR_STALLED`. |
| Sidecar death | game | `SYNCHRONIZE` handle on `sidecar.pid`, checked once per second from `frame()` | Authoritative. The game sets `sidecar.state = Absent` and raises `SIDECAR_LOST`. |
| Orderly sidecar exit | game | `sidecar.state = Closing` | Treated like a death. |

### 8.5 Restart and staleness

| Event | Detection | Effect |
|---|---|---|
| Sidecar crash or restart | Process handle, or a new `sidecar.epoch` | The game ignores sidecar slots and S2G records from the old epoch. The new sidecar resets the G2S consumer and reads the game's latest-value slots; nothing needs republishing. |
| Game crash | The sidecar's parent watch | The sidecar leaves, restores the career save and exits. The mapping disappears with the last handle. |
| Game restart | New PID and creation time | New mapping name. An orphaned sidecar cannot attach to it. |
| UE4SS "restart all mods" | `on_lua_start` fires again | The mapping stays open; `game_lua_gen + 1`; new cursors start at "now" and state is re-read. |
| New server session | The sidecar, on Welcome | `session_epoch + 1`. |
| Writer dies inside a seqlock write | `seq` stays odd | Readers get `Busy` until the slot is rewritten. |
| S2G full | The sidecar's push | The sidecar buffers up to 8,192 records in process. Beyond that it drops, counts `overflow` and bumps `resync_req`; the game raises `RESYNC` and the mods re-read state. |

**Events are hints, state is truth.** Anything whose loss would leave the game wrong is also
in a latest-value slot: deaths in the session scoreboard and the peer vitals, the match phase in
the session record, kit verdicts in `peer_kit`. Damage claims are resent at the network level
until acknowledged.

## 9. Security

- **ACL.** The game creates the mapping and both events with the security descriptor
  `O:<user SID>D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)`: owned by the current user, protected,
  full access for that user and SYSTEM only. The sidecar and the tools compare the object's owner
  SID with their own token user and refuse on a mismatch, which defeats name squatting by
  another account. `Local\` scopes the name to the logon session.
- **Read-only tools.** `ipc-dump` opens the mapping with `FILE_MAP_READ` only.
- **Data only.** No paths, code, format strings or Lua source enter the segment.
- **Bounds-safe reads.** Counts are clamped to capacities, enums validated, floats checked for
  finiteness and strings for UTF-8 before anything acts on them. A process of the same user that
  scribbles on the mapping can produce wrong game data, not a native memory fault. (Such a
  process could write the game's memory directly anyway, so it is out of scope.)
- **Peer-supplied text** (nicks, reasons, notices) is stored as length-bounded UTF-8 and only
  displayed.

## 10. Performance (measured)

**Micro-benchmarks** (`cargo test --release -p hsmp-ipc --all-features --test bench --
--ignored --nocapture`): a `local_pose` seqlock write about 0.02 µs, a read about 0.013 µs, a
ring push and pop about 0.016 µs.

**`hsmp-tools net-bench`** (real release server and sidecars, synthetic games at 60 Hz pose /
root / weapon and 20 Hz vitals, loopback through a counting proxy; reports in `bench/`):

| Clients | Server CPU per player | Sidecar CPU avg | Up pps per client | Up wire B/s per client |
|---|---|---|---|---|
| 2 | 0.42 % | 0.83 % | 79 | 36,521 |
| 4 | 0.61 % | 1.71 % | 79 | 36,508 |
| 8 | 0.61 % | 2.17 % | 80 | 36,521 |

CPU is a percentage of one core. Root, weapon and pose of one sample leave as one datagram.
Compared with the earlier JSON-bridge build measured the same way (`bench/baseline.md`), that is
60 % fewer upstream packets, 17 % fewer upstream wire bytes, 36 % less server CPU per player at 8
clients, and 43–50 % less sidecar CPU. In the synthetic game the cost of one sample is dominated
by the `SetEvent` doorbell (about 2 µs); the pose slot write including the codec v2 encode is
about 5.5 µs.

**In game** (two instances, 20 rounds on Alley and Pit, `typical` netsim):

| Metric | Lua sampling and servo | Native sampling and servo |
|---|---|---|
| Stand-in frame cost, mean / p95 | 1.11 / 1.47 ms | 0.77 / 1.09 ms |
| Pose sender, sample and write, mean / p95 | 0.67 / 0.77 ms | 0.10 / 0.26–0.27 ms |
| Latency | 65.7 ms | 65.4–66.1 ms |

Pose quality stayed within run-to-run noise. Native sampling ran about 92 % of the time (the
rest falls back to Lua between a world leaving and the next world being ready); the native servo
ran more than 99.9 % of the time. The frame-cost log line carries the used / fell-back counters.

Offline, without the engine's own call cost: a full native sample (23 bones, 22 bodies,
control; 67 `ProcessEvent` calls) takes 4.3 µs, the body servo 19.6 µs per stand-in frame, the
neutralise call 0.35 µs and the weapon servo 1.5 µs.

## 11. Tools and observability

| Tool | What |
|---|---|
| `hsmp-tools ipc-dump --pid <game> [--json] [--full] [--records N]` | Read-only live view: header, epochs, caps, heartbeats, counters, peers, slots, bus and the last ring records. `--name <mapping>` instead of `--pid`. `--follow` prints every new G2S / S2G / DevCtl record as JSON lines. `--get <slot> [--peer <id> \| --slot <n>]` prints one slot. |
| `hsmp-tools ipc-ctl --pid <game> autotest <cmd> [arg]` | Pushes one `dev_cmd` record into the DevCtl ring. Other ops: `tune <key> <value>` (HSMPAvatars and HSMPSync knobs, e.g. `native_servo 0`), `tdiag on\|off` (HSMPSync transport diagnostics). Exit 0 queued, 3 ring full (the game is not polling), 2 error. |
| `hsmp-tools ipc-game [opts] -- <sidecar.exe> <args...>` | A game stand-in for tests: creates the segment for its own PID, spawns the sidecar with `--parent-pid` and `--ipc shm:`, pumps the game side every millisecond and logs what the game sees to `--view`. `--synth` plays a live match into the game slots (used by `net-bench`). Other flags: `--caps`, `--name-file`, `--tap`, `--parent-pid`, `--stop-file`, `--no-sidecar`. |
| `hsmp-tools ipc-put --name <mapping> <name> <json\|@file>` | Writes one game-written record slot, G2S record or `dev_cmd`. |
| `hsmp-tools ipc-stress [--duration <s>] [--only 1,2,...]` | Cross-process stress (§12). |
| `hsmp-tools gen-ipc [--check]` | Code generation (§4.3). |
| Sidecar `--ipc-tap <file>` (or `HSMP_IPC_TAP`) | JSON lines `{"t", "ev", ...}` written by the `hsmp-ipc` thread: `attach`, `refuse`, `g2s` and `s2g` (every ring record, decoded), `slot` (sidecar-written record slots on change, at most 4 Hz), `play` (1 Hz per peer), `stats` (every 5 s), `detach`. For development and the gate; the gate reads sessions and command results from it. |

Mods log IPC events into `hsmp_events.jsonl`: `ipc_unavailable`, `ipc_refused`,
`ipc_backpressure`, `ipc_write_refused`.

## 12. Testing

| What | Where | Run |
|---|---|---|
| Primitives, handshake, layout, smoke | `crates/hsmp-ipc/tests/` (`primitives.rs`, `handshake.rs`, `layout.rs`, `smoke.rs`) | `cargo test -p hsmp-ipc` |
| Fuzz | `crates/hsmp-ipc/tests/fuzz.rs`: arbitrary bytes as a whole segment through every reader, proptest-driven on stable Rust. Nothing may panic or read out of bounds. | `cargo test -p hsmp-ipc` |
| Native module | `crates/hsmp-native/tests/`: `api.rs` (the whole API against an in-process fake sidecar on a real named mapping, allocation check), `records.rs` (the conformance script on the real module), `s2g_epoch.rs`, `sample.rs` (native sampling against a fake engine), `servo_parity.rs` (4000 golden vectors through HSMPAvatars' own Lua `PURE.servo`) | `cargo test -p hsmp-native` |
| Sidecar | `server/src/sidecar/ipc_shm_tests.rs`, `server/tests/sidecar_shm.rs` | `cargo test -p hsmp-server` |
| Wire decoders | `server/tests/decode_fuzz.rs`, `record_fuzz.rs`, `pose_fuzz.rs`: arbitrary bytes and mutations of valid messages of every kind | `cargo test -p hsmp-server` |
| Lua facade and mocks | `tools/hsmp-tools/lua-tests/ipc.lua`, `records.lua`, `lib/hsmp_native_records.lua`, `lib/records_conformance.lua` | `hsmp-tools lua-test ipc records` |
| Generated files | G0 `ipc_schema` | `hsmp-gate g0` |
| C++ harness | `crates/hsmp-native/cpp/test/`: a mock UE4SS.dll and `harness.exe` load the real main.dll and hsmp_lua.dll, with `harness_ipc.lua` and `harness_sample.lua` | `ctest` (see `crates/hsmp-native/README.md`) |
| In game | `crates/hsmp-native/probe/HSMPNativeProbe` (dev only, `HSMP_NATIVE_PROBE=1`) prints `IPC VERDICT attach=... abi=... slots=... ring=... bus=... frame_us=p50/p99` | deploy by hand |

`hsmp-tools ipc-stress` runs six cross-process scenarios on fresh named segments, with child
processes of the same exe that are killed by their own process handle:

| # | Scenario | Pass |
|---|---|---|
| 1 | Max-rate writer against a reader: `local_pose` seqlock, a large blob, the G2S ring | 0 torn reads, ring order continuous, payloads intact |
| 2 | Kill the writer at a random point, and a writer that dies inside a seqlock write; restart each | Death seen within 1 s, `Busy` while the slot is abandoned, recovery within 100 ms of re-attach, never old-epoch data |
| 3 | Kill and restart the reader | The writer never blocks (max loop gap under 50 ms); the restarted reader is consistent |
| 4 | Stalled consumer | The producer gets `Full`, no corruption, the ring is continuous after the stall |
| 5 | Two segments in parallel | Full isolation |
| 6 | Wrong parent, layout hash, magic | Refuse codes with exit codes 72, 71, 70 |

It exits 0 only if every scenario passes.
