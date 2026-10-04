//! The crash signature table and the CrashContext.runtime-xml parser, shared by
//! `hsmp-tools crash-triage` (which adds cdb symbolisation) and the launcher's bug reports
//! (which classify from the xml and the logs only, see [`quick`]).

use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;

// ------------------------------------------------------------------------------------------
// Known-signature table (ordered: the first match wins).
// ------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThreadReq {
    Any,
    Game,
    NotGame,
}

pub struct Sig {
    pub id: &'static str,
    pub title: &'static str,
    pub cause: &'static str,
    pub fix: &'static str,
    pub confidence: &'static str,
    /// Must match the joined frames (cdb symbols, or "Module+0xoff" without cdb).
    pub frames: Option<&'static str>,
    /// Must match the error text: exception description + xml ErrorMessage/CrashType + log tail.
    pub text: Option<&'static str>,
    /// Must match the faulting module (first frame's module).
    pub module: Option<&'static str>,
    /// Must NOT match the joined frames.
    pub not_frames: Option<&'static str>,
    /// Must match the crashing thread's name.
    pub thread_name: Option<&'static str>,
    /// Requires another non-game thread to be executing Lua at crash time.
    pub offthread: bool,
    pub thread: ThreadReq,
}

const LUA_FRAMES: &str = r"(?i)\b(lua_\w+|luaH_\w+|luaV_\w+|luaD_\w+|luaG_\w+|luaB_\w+|__index|__newindex|LuaMadeSimple)";

pub const SIGS: &[Sig] = &[
    Sig {
        id: "HANG_STALL",
        title: "Hang / stall detected by the engine watchdog",
        cause: "The game thread did not tick for the stall threshold (IsStall) or the heartbeat thread fired.",
        fix: "Look at the GameThread stack; check hitch events in hsmp_log; DoD-2 stall rule.",
        confidence: "high",
        frames: None,
        text: Some(r"(?i)IsStall:true|\bhang detected\b|FThreadHeartBeat"),
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "SOFTOBJECT_PARAM_READ",
        title: "SoftObject/SoftClass property or TSoftObjectPtr hook param read from Lua",
        cause: "UE4SS 3.0.1 push_softobjectproperty memcpys a null FSoftObjectPath string (e.g. Level:get() in an OpenLevelBySoftObjectPtr hook, obj[softprop]); uncatchable AV.",
        fix: "Never :get() soft params / read Soft* properties; treat as opaque (docs/development/lua-mods.md; check_unsafe U1/U2).",
        confidence: "high",
        frames: Some(r"(?i)push_softobjectproperty|push_softclassproperty|FSoftObjectPath::operator=|FSoftObjectPtr|TSoftObjectPtr|SoftObjectProperty"),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "LEADER_POSE_SKSKELETON",
        title: "SetLeaderPoseComponent on a stand-in's SK_Skeleton",
        cause: "Calling SetLeaderPoseComponent (from Lua) on SK_Skeleton of a Willie stand-in crashes in the leader-pose bone map.",
        fix: "Do not call SetLeaderPoseComponent; drive the visible Mesh (docs/development/lua-mods.md; check_unsafe U3).",
        confidence: "high",
        frames: Some(r"(?i)SetLeaderPoseComponent|SetMasterPoseComponent|UpdateLeaderBoneMap|LeaderBoneMap|LeaderPose"),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "LUA_VM_OFFTHREAD",
        title: "Lua VM corrupted by an off-thread callback",
        cause: "LoopAsync / ExecuteWithDelay / RegisterKeyBindAsync callbacks run on a UE4SS worker thread while game-thread Lua runs in the same state: the VM is corrupted (lua_next, __index on garbage userdata). Evidence: the crashing thread is not the GameThread, or another (unnamed UE4SS) thread was executing Lua at the same moment.",
        fix: "Game-thread shim in every mod (LoopInGameThreadWithDelay / ExecuteInGameThreadWithDelay; see docs/development/lua-mods.md); check_unsafe U4.",
        confidence: "high",
        frames: Some(LUA_FRAMES),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::NotGame,
    },
    Sig {
        id: "LUA_VM_OFFTHREAD",
        title: "Lua VM corrupted by an off-thread callback",
        cause: "LoopAsync / ExecuteWithDelay / RegisterKeyBindAsync callbacks run on a UE4SS worker thread while game-thread Lua runs in the same state: the VM is corrupted (lua_next, __index on garbage userdata). Evidence: the crashing thread is not the GameThread, or another (unnamed UE4SS) thread was executing Lua at the same moment.",
        fix: "Game-thread shim in every mod (LoopInGameThreadWithDelay / ExecuteInGameThreadWithDelay; see docs/development/lua-mods.md); check_unsafe U4.",
        confidence: "high",
        frames: Some(LUA_FRAMES),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: true, thread: ThreadReq::Any,
    },
    Sig {
        id: "STALE_UOBJECT_AFTER_TRAVEL",
        title: "Stale UObject touched from Lua after a level change",
        cause: "A UObject cached before OpenLevel (round reset re-opens the arena) is read/written afterwards: UStruct::FindProperty <- __newindex <- luaV_finishset <- process_simple_actions; pcall cannot catch it.",
        fix: "World guard shared/hsmp_wg.lua (WG.check / WG.on_drop / WG.token+same; see docs/development/lua-mods.md); check_wg.",
        confidence: "high",
        frames: Some(r"(?i)UStruct::FindProperty|FindPropertyByName|luaV_finishset|__newindex|UObjectBase::GetClass|UObject::GetClassPrivate"),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Game,
    },
    Sig {
        id: "LUA_VM_CORRUPTION",
        title: "Lua VM internals faulted on the game thread (corrupted state / garbage userdata)",
        cause: "The fault is inside the Lua VM itself (lua_getiuservalue / get_userdata / lua_next / luaH_*): the state or a userdata was corrupted earlier, classically by off-thread Lua (pre-shim builds) or by a userdata whose UObject was freed.",
        fix: "Confirm every mod has the game-thread shim (check_unsafe U4) and the world guard (check_wg); see docs/development/lua-mods.md.",
        confidence: "medium",
        frames: Some(r"(?i)\A[^\n]*(lua_getiuservalue|get_userdata|lua_next|luaH_\w+|index2value|luaC_\w+|lua_rawget\w*|luaV_finishget|lua_touserdata|lua_type\b|traverse\w+|propagatemark|singlestep|reallymarkobject|freeobj)"),
        text: None,
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Game,
    },
    Sig {
        id: "LUA_UOBJECT_ACCESS",
        title: "Fault inside UE4SS while Lua touched a UObject (game thread)",
        cause: "Lua on the game thread faulted in UE4SS reflection code (ProcessEvent, construct_fname = GetFName()/GetFullName(), metamethods on AActor): almost always a stale or half-destroyed UObject (world teardown, pooled Willie) reached through a cached reference or a delayed callback.",
        fix: "Re-fetch objects each callback and run WG.check() / WG.same(token) first; inspect the Lua call site in UE4SS.log; check_unsafe U5 + check_wg.",
        confidence: "medium",
        frames: Some(LUA_FRAMES),
        text: None,
        module: Some(r"(?i)^(UE4SS|VCRUNTIME140|ucrtbase)$"),
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Game,
    },
    Sig {
        id: "NATIVE_FROM_LUA_CALL",
        title: "Engine code crashed inside a UFunction called from Lua",
        cause: "A UFunction invoked from Lua (game thread) faulted in engine code: a bad argument, an object in the wrong state (pooled/dying Willie, stand-in mesh), or a teardown race.",
        fix: "Find the call in UE4SS.log just before the crash; guard with WG.check(); check_unsafe U3 lists the known-bad calls.",
        confidence: "medium",
        frames: Some(LUA_FRAMES),
        text: None,
        module: Some(r"(?i)HalfswordUE5"),
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Game,
    },
    Sig {
        id: "UE4SS_HOOK_CRASH",
        title: "Crash in a UE4SS function hook / detour",
        cause: "A RegisterHook detour (PolyHook / TDetourInstance) faulted outside Lua: hooked function signature mismatch, hook on a dying object, or a hook fired during teardown.",
        fix: "Check which RegisterHook paths are installed; prefer post-hooks that do work synchronously; avoid hooks on engine internals.",
        confidence: "medium",
        frames: Some(r"(?i)PolyHook|PLH::|TDetourInstance|UnrealScriptFunctionHook|FirePreCallbacks|FirePostCallbacks"),
        text: None,
        module: Some(r"(?i)^(UE4SS|VCRUNTIME140|ucrtbase)$"),
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "GPU_DEVICE_REMOVED",
        title: "GPU device removed / hung (driver)",
        cause: "D3D12 device removed or hung (driver crash, TDR, overclock, out of VRAM).",
        fix: "Not an HSMP bug: update the driver / lower settings; re-run. Two instances on one GPU raise VRAM pressure.",
        confidence: "high",
        frames: None,
        text: Some(r"(?i)DXGI_ERROR_DEVICE_(REMOVED|HUNG|RESET)|D3D12.*(removed|hung)|GPU (crash|hang)|nvwgf2um|amdxc64|igxelpicd|d3d12core"),
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "OUT_OF_MEMORY",
        title: "Out of memory",
        cause: "An allocation failed (FMallocBinned / OOM); a leak over many rounds or two instances on one box.",
        fix: "Check the soak memory slope (private bytes per round); look for growing Lua tables / widgets per round.",
        confidence: "high",
        frames: None,
        text: Some(r"(?i)out of memory|OutOfMemory|Ran out of memory|FMallocBinned\w*::OutOfMemory|MallocBinned.*fail"),
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "ASSERTION",
        title: "Engine assertion / ensure / fatal error",
        cause: "check()/ensure()/LowLevelFatalError in engine code; the message names the condition.",
        fix: "Read error_message; map the asserted condition to the HSMP action that preceded it (UE4SS.log).",
        confidence: "high",
        frames: None,
        text: Some(r"(?i)Assertion failed|IsAssert:true|IsEnsure:true|CrashType:(Assert|Ensure)|LowLevelFatalError|Fatal error!"),
        module: None,
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "EXEC_BAD_ADDRESS",
        title: "Execute access violation (call/jump to a non-code address)",
        cause: "The CPU jumped into data (heap / a module's data range): a call through a dangling vtable or function pointer, typically a destroyed object still referenced by the engine or a stale delegate.",
        fix: "Note the caller frame (second frame); correlate with what was destroyed just before (travel, Willie hide/destroy, widget removal).",
        confidence: "medium",
        frames: None,
        text: Some(r"(?im)EXCEPTION_ACCESS_VIOLATION 0x[0-9a-f]+\s*$|Access violation - code c0000005.*execute"),
        module: None,
        not_frames: None,
        thread_name: None,
        offthread: false,
        thread: ThreadReq::Any,
    },
    Sig {
        id: "HSMP_NATIVE_DLL",
        title: "Fault in an HSMP native module",
        cause: "Crash inside an HSMP native DLL (HSMPNative / hsmp_client / hsmp_lua).",
        fix: "Symbolise with that DLL's PDB (add --sympath) and fix it in the code that DLL belongs to.",
        confidence: "high",
        frames: None,
        text: None,
        module: Some(r"(?i)hsmp"),
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    // The round-reset crash (docs/development/crash-rr.md). Exact site in the 5.4.4-2705 shipping exe: RVA
    // 0x4a23c64 (cdb: src_strerror+0x2964e84) = the Runtime Vertex Paint plugin's game-thread
    // "task finished" lambda: `Results.TaskDuration = TaskWorld->TimeSeconds - TaskStartTime`
    // (movsd xmm1,[rax+6D0h] with rax = the task's raw TaskWorld). Callstack hash
    // F1748EA9C974C109B67F98A9BFDAA5FD3E66A1FE.
    Sig {
        id: "RVP_TASK_AFTER_TRAVEL",
        title: "Runtime Vertex Paint task finished after its world was freed (round-reset crash)",
        cause: "The VertexPaintDetection plugin's task queue lives in a GameInstance subsystem and outlives the level. A blood/wound paint task (a bleeding Willie queues ~100/s) still queued or running when OpenLevel tore the arena down finished afterwards; its game-thread callback read TimeSeconds of the freed old UWorld. ~1 s after the round reload, no Lua frame.",
        fix: "Fixed by shared/hsmp_rvp.lua: every Director travel closes the RVP queue (MaxAmountOfAllowedTasksPerMesh=0), purges it and waits until it is quiet (Dir:open 'held'). If this fires again: a travel bypassed the Director (native OpenLevel / legacy_travel) or the hold timed out (rvp_hold_timeout event).",
        confidence: "high",
        frames: Some(r"(?i)HalfswordUE5-Win64-Shipping\+0x4a23c64\b|src_strerror\+0x2964e84\b"),
        text: None,
        module: None,
        not_frames: Some(LUA_FRAMES),
        thread_name: None,
        offthread: false,
        thread: ThreadReq::Game,
    },
    // The same crash, worker side: an RVP plugin thread reading a freed mesh's vertex data
    // (movzx eax,byte ptr [rdx+rax] at exe+0x1e28f4f, faulting at a page boundary).
    Sig {
        id: "RVP_WORKER_AFTER_TRAVEL",
        title: "Runtime Vertex Paint worker faulted (task on a mesh of a torn-down world)",
        cause: "A 'Runtime Vertex Paint and Detection Plugin Thread' faulted in game code with no Lua on any stack: a paint/detect task kept running on the vertex data of a mesh freed by a level change (same root cause as RVP_TASK_AFTER_TRAVEL).",
        fix: "shared/hsmp_rvp.lua quiesces RVP before every Director travel; check the travel that preceded the crash went through Dir:open (no 'held' / quiesce log line = it bypassed the guard).",
        confidence: "medium",
        frames: None,
        text: None,
        module: None,
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)Vertex Paint"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    // The known IoDispatcher pak-read crash (docs/development/halfsword/io-dispatcher-crash.md).
    // Exact site in the 5.4.4-2705 shipping exe: RVA 0x21e5ca4 =
    // FPakAsyncReadFileHandle::ReadRequest's `++GetBlock(i).RefCount` (cdb: src_strerror+0x126ec4),
    // reached from the package-resource IoDispatcher backend (EIoChunkType 13). PCallStackHash
    // 44106A9DC8756FBE63C328366971E9612B82C734 for both known dumps.
    Sig {
        id: "PAK_ASYNC_READ_OOB",
        title: "IoDispatcher: .pak async read past the end of a compressed entry (FPakAsyncReadFileHandle::ReadRequest, block index out of range)",
        cause: "A bulk-data (EIoChunkType::PackageResource) read of MetaHumans/Taro/MaleHair/Hair/Hair_M_SideSweptFringe.ubulk (Willie_BP's Hair groom; 339 blocks) starts past the end of the entry; Shipping strips ReadRequest's check(), so Blocks[339] is heap garbage and RefCount++ faults. Trigger: in the first arena of a process, a Willie hidden from BeginPlay is shown later (HSMPLoadout's invisible dressing, e.g. the 3 s safety reveal when the kit is late); reproduced in single-player with the dev probe. r.HairStrands.Streaming=0 does not prevent it. The game thread keeps running ~30-40 s after the fault, so logs show events after the crash and the next level load hangs.",
        fix: "docs/development/halfsword/io-dispatcher-crash.md: r.HairStrands.UseCardsInsteadOfStrands=1 (Engine.ini [SystemSettings], the Steam launch option, the HSMPMatch runtime cvar) means the strands data is never read; check that the UE4SS log reads it back as 1. HSMPLoadout's hair warm-up (HAIR_WARM_S) is defence in depth.",
        confidence: "high",
        frames: Some(r"(?i)HalfswordUE5-Win64-Shipping\+0x21e5ca4\b"),
        text: None,
        module: None,
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)^IoDispatcher$"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    Sig {
        id: "PAK_ASYNC_READ_OOB",
        title: "IoDispatcher-thread access violation (pak-read crash family, site not yet confirmed)",
        cause: "The IoDispatcher thread faulted in game code with no Lua on any stack. The one root-caused instance is an out-of-range .pak async read; a different RVA means another site on the same thread: compare frames with docs/development/halfsword/io-dispatcher-crash.md.",
        fix: "Symbolise as in docs/development/halfsword/io-dispatcher-crash.md (.fnent on each frame, look for the FCachedAsyncBlock layout); apply its Lua mitigations; add the new RVA here once confirmed.",
        confidence: "medium",
        frames: None,
        text: Some(r"(?i)ACCESS_VIOLATION|Access violation|c0000005"),
        module: Some(r"(?i)HalfswordUE5"),
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)^IoDispatcher$"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    Sig {
        id: "PHYSICS_DYING_WORLD",
        title: "Native worker-thread AV during a level change (suspected physics/streaming on a dying world)",
        cause: "An engine worker thread (task graph / Chaos / async loading / thread pool) faulted in game code with no Lua on any stack: work on bodies/assets of the world being torn down, e.g. bodies left simulating / constrained by Lua-driven PhysicsHandles when the level changed. (IoDispatcher-thread faults are PAK_ASYNC_READ_OOB.)",
        fix: "Before travel stop simulation / release PhysicsHandles on every Lua-driven body (OpenLevel pre-hook), never touch bodies after WG drop; correlate the crash time with the last travel event.",
        confidence: "low",
        frames: None,
        text: Some(r"(?i)ACCESS_VIOLATION|Access violation|c0000005"),
        module: Some(r"(?i)HalfswordUE5|Chaos|PhysX|PhysicsCore"),
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)Worker|TaskGraph|Chaos|Physic|AsyncLoading|ThreadPool"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    Sig {
        id: "RENDER_THREAD_NATIVE",
        title: "Native fault on the render / RHI thread",
        cause: "RenderThread / RHI faulted in engine or driver code (no Lua involved): driver, shader/PSO, or a render proxy of an actor destroyed mid-frame.",
        fix: "Update the GPU driver; if it repeats around travel or Willie hide/destroy, correlate with hsmp_log events.",
        confidence: "low",
        frames: None,
        text: None,
        module: None,
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)RenderThread|RHI"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    Sig {
        id: "PLUGIN_THREAD_NATIVE",
        title: "Native fault on a game-plugin thread",
        cause: "A plugin thread of the game (e.g. Runtime Vertex Paint and Detection) faulted; not HSMP code.",
        fix: "Game/plugin bug; note the build and whether HSMP was loaded. Re-run.",
        confidence: "low",
        frames: None,
        text: None,
        module: None,
        not_frames: Some(LUA_FRAMES),
        thread_name: Some(r"(?i)Plugin|Vertex Paint"),
        offthread: false,
        thread: ThreadReq::NotGame,
    },
    Sig {
        id: "UE4SS_FAULT_OTHER",
        title: "Unclassified fault inside UE4SS",
        cause: "UE4SS faulted outside the known Lua/hook signatures.",
        fix: "Read the symbolised frames; add a signature here once root-caused.",
        confidence: "low",
        frames: Some(r"(?i)\bUE4SS[!+]"),
        text: None,
        module: Some(r"(?i)^(UE4SS|VCRUNTIME140|ucrtbase)$"),
        not_frames: None,
        thread_name: None, offthread: false, thread: ThreadReq::Any,
    },
    Sig {
        id: "GAME_THREAD_NATIVE",
        title: "Unclassified native fault on the game thread",
        cause: "The game thread faulted in engine code with no Lua frame on the stack.",
        fix: "Correlate with the hsmp_log events just before the crash time; add a signature once root-caused.",
        confidence: "low",
        frames: None,
        text: None,
        module: Some(r"(?i)HalfswordUE5"),
        not_frames: Some(LUA_FRAMES),
        thread_name: None, offthread: false, thread: ThreadReq::Game,
    },
];

pub const UNKNOWN: Sig = Sig {
    id: "UNKNOWN",
    title: "Unknown crash",
    cause: "No known signature matched.",
    fix: "Symbolise and root-cause; then add a signature to crates/hsmp-diag/src/triage.rs.",
    confidence: "none",
    frames: None,
    text: None,
    module: None,
    not_frames: None,
    thread_name: None, offthread: false, thread: ThreadReq::Any,
};

/// Everything the classifier looks at.
#[derive(Default, Debug, Clone)]
pub struct Evidence {
    pub frames: Vec<String>,
    /// The crashing thread's frames as "Module+0xRVA" (CrashContext xml); matched by `frames`
    /// regexes after the symbolised frames, so a signature can key on an exact code address.
    pub rva_frames: Vec<String>,
    pub module: String,
    pub thread: String,
    /// Exception description + error message + crash type + flags + log tail.
    pub text: String,
    /// Non-game threads (other than the crashing one) that were executing Lua at crash time.
    pub lua_threads: Vec<String>,
}

pub fn is_game_thread(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("GameThread")
}

pub fn classify(ev: &Evidence) -> &'static Sig {
    let mut frames = ev.frames.join("\n");
    if !ev.rva_frames.is_empty() && ev.rva_frames != ev.frames {
        frames.push('\n');
        frames.push_str(&ev.rva_frames.join("\n"));
    }
    for s in SIGS {
        let ok_re = |re: Option<&str>, hay: &str| re.map(|r| Regex::new(r).unwrap().is_match(hay)).unwrap_or(true);
        if !ok_re(s.frames, &frames) || !ok_re(s.text, &ev.text) || !ok_re(s.module, &ev.module) {
            continue;
        }
        if let Some(nf) = s.not_frames {
            if Regex::new(nf).unwrap().is_match(&frames) {
                continue;
            }
        }
        if !ok_re(s.thread_name, &ev.thread) || (s.offthread && ev.lua_threads.is_empty()) {
            continue;
        }
        let thread_ok = match s.thread {
            ThreadReq::Any => true,
            ThreadReq::Game => is_game_thread(&ev.thread),
            ThreadReq::NotGame => !is_game_thread(&ev.thread),
        };
        if thread_ok {
            return s;
        }
    }
    &UNKNOWN
}

pub fn sig_json(s: &Sig) -> Value {
    json!({"id": s.id, "title": s.title, "cause": s.cause, "fix": s.fix, "confidence": s.confidence})
}

// ------------------------------------------------------------------------------------------
// CrashContext.runtime-xml
// ------------------------------------------------------------------------------------------

#[derive(Default, Debug, Clone)]
pub struct XmlInfo {
    pub error_message: String,
    pub crash_type: String,
    pub is_assert: bool,
    pub is_ensure: bool,
    pub is_stall: bool,
    pub seconds_since_start: Option<i64>,
    pub crashed_thread: String,
    /// Crashed thread's stack as "Module+0xoff".
    pub frames: Vec<String>,
    /// PCallStackHash: identical stacks (same build) hash the same.
    pub callstack_hash: String,
}

fn tag<'a>(x: &'a str, t: &str) -> Option<&'a str> {
    let open = format!("<{t}>");
    let close = format!("</{t}>");
    let s = x.find(&open)? + open.len();
    let e = x[s..].find(&close)? + s;
    Some(&x[s..e])
}

fn unxml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn callstack_frames(cs: &str) -> Vec<String> {
    let re = Regex::new(r"^\s*(\S+)\s+0x[0-9a-fA-F]+\s*\+\s*([0-9a-fA-F]+)").unwrap();
    cs.lines()
        .filter_map(|l| re.captures(l).map(|c| format!("{}+0x{}", &c[1], c[2].to_ascii_lowercase())))
        .collect()
}

pub fn parse_xml(x: &str) -> XmlInfo {
    let b = |t: &str| tag(x, t).map(|v| v.trim().eq_ignore_ascii_case("true")).unwrap_or(false);
    let mut info = XmlInfo {
        error_message: tag(x, "ErrorMessage").map(|s| unxml(s.trim())).unwrap_or_default(),
        crash_type: tag(x, "CrashType").map(|s| s.trim().to_string()).unwrap_or_default(),
        is_assert: b("IsAssert"),
        is_ensure: b("IsEnsure"),
        is_stall: b("IsStall"),
        seconds_since_start: tag(x, "SecondsSinceStart").and_then(|s| s.trim().parse().ok()),
        callstack_hash: tag(x, "PCallStackHash").map(|s| s.trim().to_string()).unwrap_or_default(),
        ..Default::default()
    };
    // <Thread><CallStack>..</CallStack><IsCrashed>true</IsCrashed>..<ThreadName>X</ThreadName></Thread>
    let mut rest = x;
    while let Some(i) = rest.find("<Thread>") {
        let body_end = rest[i..].find("</Thread>").map(|e| i + e).unwrap_or(rest.len());
        let body = &rest[i..body_end];
        if tag(body, "IsCrashed").map(|v| v.trim() == "true").unwrap_or(false) {
            info.crashed_thread = tag(body, "ThreadName").map(|s| unxml(s.trim())).unwrap_or_default();
            info.frames = strip_dispatch_frames(callstack_frames(tag(body, "CallStack").unwrap_or("")));
            break;
        }
        rest = &rest[body_end.min(rest.len())..];
        if body_end >= x.len() {
            break;
        }
    }
    if info.frames.is_empty() {
        info.frames = strip_dispatch_frames(callstack_frames(tag(x, "PCallStack").unwrap_or("")));
    }
    info
}


/// Drop the crash-reporter frames at the top of an xml stack (NtWaitForSingleObject ...
/// KiUserExceptionDispatcher): keep what follows the last ntdll frame among the first 10.
pub fn strip_dispatch_frames(frames: Vec<String>) -> Vec<String> {
    if !frames.first().map(|f| f.to_ascii_lowercase().starts_with("ntdll")).unwrap_or(false) {
        return frames;
    }
    let last = frames.iter().take(10).rposition(|f| f.to_ascii_lowercase().starts_with("ntdll"));
    match last {
        Some(i) if i + 1 < frames.len() && i > 0 => frames[i + 1..].to_vec(),
        _ => frames,
    }
}

pub fn frame_module(f: &str) -> String {
    f.split(['!', '+']).next().unwrap_or("").trim().to_string()
}

/// One crash folder, classified without a debugger (the xml stack and the logs only).
pub fn quick(dir: &Path) -> Value {
    let xml = std::fs::read(dir.join("CrashContext.runtime-xml")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let xi = parse_xml(&xml);
    let mut tail = String::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("log")) {
                if let Ok(b) = std::fs::read(&p) {
                    let t = String::from_utf8_lossy(&b);
                    let lines: Vec<&str> = t.lines().collect();
                    tail += &lines[lines.len().saturating_sub(60)..].join("\n");
                    tail.push('\n');
                }
            }
        }
    }
    let module = xi.frames.first().map(|f| frame_module(f)).unwrap_or_default();
    let ev = Evidence {
        frames: xi.frames.clone(),
        rva_frames: xi.frames.clone(),
        module: module.clone(),
        thread: xi.crashed_thread.clone(),
        text: format!("{}\nCrashType:{}\nIsAssert:{}\nIsEnsure:{}\nIsStall:{}\n{tail}", xi.error_message, xi.crash_type, xi.is_assert, xi.is_ensure, xi.is_stall),
        lua_threads: vec![],
    };
    json!({
        "dir": dir.file_name().map(|n| n.to_string_lossy().into_owned()),
        "error_message": xi.error_message.lines().next().unwrap_or(""),
        "crash_type": xi.crash_type,
        "thread": xi.crashed_thread,
        "module": module,
        "seconds_since_start": xi.seconds_since_start,
        "frames": xi.frames.iter().take(12).cloned().collect::<Vec<_>>(),
        "callstack_hash": xi.callstack_hash,
        "signature": sig_json(classify(&ev)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_classifies_a_crash_folder_from_its_xml() {
        let d = std::env::temp_dir().join(format!("hsmp_diag_triage_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let xml = "<FGenericCrashContext><RuntimeProperties><ErrorMessage>Unhandled Exception: EXCEPTION_ACCESS_VIOLATION reading address 0x0000000000000010</ErrorMessage><CrashType>Crash</CrashType>\
<Threads><Thread><CallStack>HalfswordUE5-Win64-Shipping 0x00007ff700000000 + 4a23c64 \n</CallStack><IsCrashed>true</IsCrashed><ThreadName>GameThread</ThreadName></Thread></Threads></RuntimeProperties></FGenericCrashContext>";
        std::fs::write(d.join("CrashContext.runtime-xml"), xml).unwrap();
        let q = quick(&d);
        assert_eq!(q["signature"]["id"], "RVP_TASK_AFTER_TRAVEL", "{q}");
        assert_eq!(q["thread"], "GameThread");
        assert!(q["error_message"].as_str().unwrap().contains("ACCESS_VIOLATION"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
