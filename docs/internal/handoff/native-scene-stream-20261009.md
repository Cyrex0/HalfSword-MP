# Native full-scene encoding checkpoint

## Latest actual: 5339682b

The complete lossless codec/stream/lifecycle checkpoint passes full G0: 74 Lua
suites, 1355 Rust checks in 79 binaries and clippy with 0 errors/13 warnings.
It is pushed to dev and cleanly RequireG0 deployed; all 393 deployed hashes
match HEAD/binaries/G0 `5339682ba8c33a0385666c575be3a613628ce04b`.
The first gate's sole test-only Instant arithmetic lint was corrected with
checked_sub and its existing stale-scene assertion; the required full retry passes.

Actual `test-results/20261009-133011-adbf6a-native-host` uses source UUID
96ce45e0-0c7b-464f-bdc9-991de1e44e01, supervisor 39560, authority 36552 and
normal rendered clients 31508/44272. NativeReady is 15.668s. Entity1 original
epoch 6164706162233355395/incarnation5/directory6/revision1/frame1 capture
runs 16.273s to 39.031s (22.758s). Both complete 49-component harvests pass,
including all four empty skeletal holders. NativeBind refuses `armor slot
binding` at 39.035s. Encoding statistics validate first, so actual compact/raw
bytes and the 512KiB encoded capacity remain unproved; the old Lua-to-JSON
staging refusal is no longer first. Entity2 capture, canonical/mirror frames
and active input [0,0] remain unverified. No scene reached delivery, so this
run cannot establish network latency or justify changing the part scheduler.

The unchanged 65s readiness gate fails. Both clients stop; the authority begins
another capture after directory replacement and misses graceful shutdown. Its
exact owned supervisor fallback stops it. Last own phase is seq1338 at75.108s,
mesh_census/pass2/entity1/incarnation6/directory8. All four original PIDs are
independently absent; Shipping games[], 20/20 original save hashes unchanged,
crash[]/unobservedchildren[]. Full native log, exact 860-line authority36552
trace and native_operator_summary.json are preserved.

Both original windows were qualified and moved early, with initial evidence
preserved. Their outer bounds were 896x566; client2's right edge remained16px
on the primary. A later 880x527 fit occurred after exit and was unqualified,
with no window action. Full secondary containment is still unproved. The
ignored operator helper now performs the requested fit immediately next run.

Next fixes: match construction armor map semantics from actual cooked
Map_Values/independent passport Slot evidence, preserving every row and field;
emit precise copied slot context and bounded prevalidation size diagnostics;
cancel long captures at throttled admission boundaries and perform native
teardown only after unwinding. Then focused tests/independent review, combined
G0/dev checkpoint and another exact deployed actual scenario. No source
performance or scene delivery gain is inferred from this run.

The construction fix is implemented from the native evidence in
`native-armor-slot-proof-20261009.md`: all construction map keys and passport
slots remain independent copied values; strict live bindings remain unchanged.
Refusals now report copied table/row/key/passport slot/class plus bounded planned
encoding statistics before semantic admission. Invalid recipes cannot publish.
The production Lua plain/signature path also now uses the 512KiB token-derived
value/table budget, replacing its remaining old16000 ceiling. Focused descriptor
23/raw parser11/Lua source257/syntax2 checks and independent Sol review pass.
The wide Lua capture retains all32768 bone occurrences and detects tail mutation.
The fixture changes retain native f32 colour widths. No new actual success is
inferred from these offline results.

Worker cancellation is also implemented and independently reviewed: stop-file
and parent checks are throttled at250ms admission boundaries and only latch a
reason/fault. Source capture unwinds and closes its original native scope before
the outer loop performs the existing controller/host/Director shutdown once.
Focused worker247 assertions/syntax2 pass, including stop and unavailable parent
mid-capture, refusal of the next getter/registration and scope-end-before-teardown.
Original world/lifetime/controller checks remain. Actual graceful stop remains
pending; these checks do not establish playable registration or scene delivery.

## Previous actual: 0039aacd

Latest actual checkpoint0039aacde42c5b0a4c69c2de32aaf3f0df6a160f passed full
G0 (74 Lua suites/1333 Rust checks in79 binaries, clippy0 errors/12 warnings),
dev push and exact clean RequireG0 deployment. Native source/helper/FFI review
was closed with focused C++925 /W4 /WX and source Lua243/syntax3, scopes32,
parser8/presentation8/descriptor12. These offline checks do not prove gameplay.

Actual run `test-results/20261009-122114-c48890-native-host` used source UUID
552c6bcd-c83d-42f9-a825-176aaefb92f0, authority39116/supervisor23932 and normal
rendered clients42056/43700. NativeReady15.747s. Original entity1 has epoch
1413459191929052231/incarnation5/directory6. GripSk40 at28.171s, GuardSk43 at
28.477s, HeadSk45 at28.723s and PommelSk47 at28.965s all prove native_empty_skeletal,
both assets absent, no override, LOD count0, material count/mask0 and bone count0.
All49 component rows complete in both harvests29.906s/42.189s. Adapter/source
capture finishes42.222s; native registration refuses `source recipe byte bound`
at42.223s/own seq893. Exact raw JSON length was not emitted. This closes the
actual empty skeletal source-collector blocker, while native bulk capture,
client mirror creation/readback and playable inputs remain unverified.

Canonical/mirror frames0, sampled0 and active inputs[0,0]. The authority misses
its graceful stop deadline after the expensive capture and the supervisor stops
only the original owned process. No clean source-stopped event is present.
Both clients qualify suppression/isolation, remain wait_scene and then stop.
All4 original PIDs are independently absent, no shipping game remains,20/20
original save hashes match, crash[]/unobservedchildren[]. Full HSMPNative.log,
exact authority39116 trace524 lines and native_operator_summary.json are saved.
Live window rectangles were missed before exit; secondary placement remains
unproved and no window movement was performed in this run.

Root cause: native_descriptor_binding's Lua→JSON staging applies60KiB before a
typed SourceRecipe exists; canonical_bytes/decode_recipe independently impose
the same old JSON limit. Actual first-entity fields also prove600 bone transforms
and a V3 lower bound55,869 bytes before core/material payloads. Exact second-
entity and recipe lengths remain unobserved. The full source cannot be reduced
by omitting body/gear components, materials, collision fields or native bone data.

Implementation in progress requires capability27 NATIVE_SCENE_STREAM, compact
dictionary recipes with unchanged logical schema6 and a versioned binary codec,
direct bounded LuaValue staging, and trusted scalar size diagnostics. The user
explicitly authorizes increased data budgets on2026-10-09: give complete scenes
more capacity, preserve high resolution and speed, and use only lossless encoding.
The encoded recipe budget therefore increases to512KiB with bounded descriptor
parts0AC2;64KiB remains an individual transport-record limit. Preserve
depth24 and all typed field bounds. The value/table node budget derives from
the512KiB encoded budget (each token consumes at least one byte); object keys
are dictionary references rather than extra nodes. This replaces the old16000
node ceiling without dropping fields. Dictionary and decoded allocation bounds
derive from the accepted encoded budget.
No inferred defaults or lost duplicate occurrences. Decode rejects malformed,
duplicate, unknown or out-of-bound data before aggregate allocation.

Lossless render manifests/byte parts retain internal RenderWorldV3 and every
native f64 bit. Old single-message V3 helpers keep64KiB refusal. Source preflight,
Bridge publication and relay must use bounded logical-frame validation together.
Complete logical scene capacity derives from accepted recipe counts and can
exceed100KiB; never shrink resolution to fit a packet. Recipe dictionaries run
only on metadata capture, while pose frame values remain raw. Measure actual
processing/transmission costs; no zero-latency claim follows from losslessness.
Per-peer one pinned active batch plus one replaceable pending latest frame closes
backpressure and supersession starvation. Receiver ADMITTED precedes parts and
requires current recipes/derived allocation bounds; COMPLETE follows complete
validated atomic publication. Neither acknowledgment can grant native MirrorReady.
Native complete-scene readback/final original guards remain mandatory.

Startup READY complete scenes remain generation-pinned through native preparation.
LIVE input keeps the existing250ms freshness requirement after apply and before
send. Keep inert mirrors through transient gaps; clear only actual identity/world
generation changes. Temporary-expiry recreation and post-apply input freshness
omissions are fixed with73 focused Lua assertions; actual validation is pending.
Safe performance work must
follow actual stage measurements and retain all final native proofs.

Source changes are authorized; no new actual run/full G0 until the coherent
codec/scheduler/lifecycle checkpoint passes focused checks and independent review.
Focused verification and independent reviews are closed. Core lifecycle73 Lua
assertions and syntax2 pass; network/server native64, presentation8, raw parser10,
source scopes32 and expanded descriptor21 checks pass. Prior native capability
bits20–26 without27 are refused for both presenting PvP and diagnostic peers.
World-only diagnostic peers retain their existing admission behavior.

Offline authenticated UDP delivers a complete125,362-byte synthetic scene
atomically in311ms in the debug/local endpoint test. This is neither native nor
release latency evidence. The expanded64-component/32,768-bone recipe fixture
retains all fields: raw JSON2,142,406 bytes, compact393,603, nodes103,422,
dictionary23,772 and tokens369,831. The smaller synthetic49-component/600-bone
fixture is74,449 raw JSON bytes/24,511 compact; debug metadata encode mean is
5,345 microseconds over8 iterations and has no material slots. These fixtures
prove capacity/bit retention, not actual roster size or gameplay parity.

Next: one coherent full G0/dev push, exact clean RequireG0 deployment, then the
normal100-second native host scenario with unchanged65-second readiness,
10-second observation and250ms LIVE freshness bounds. Capture both owned client
window rectangles early and verify secondary placement. Measure real recipe,
frame, mirror and input processing/delivery evidence before further optimization.
No main PR, merge, tag or headless/co-op release is authorized by these results.
