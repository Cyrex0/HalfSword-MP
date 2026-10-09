# Native full-scene encoding checkpoint

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
