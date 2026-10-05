//! Combat and vitals (kinds `0x03xx`, caps `COMBAT` / `VITALS`), ABI 2 / protocol v8.
//!
//! One record per datum, end to end:
//! - [`Damage`] (+ [`DamageDelta`] rows): the attacker's claim. The game sends it G2S
//!   (`IPC.send("damage", t)`, `cid` = its claim id); the attacker's sidecar fills `hit_id`,
//!   and `age_ms` in place and resends it C2S until a final verdict. Original match, round
//!   and both pawn lives remain unchanged. The server
//!   forwards the approved record to the victim's owner as `damage_in` (alias kind, `WireHdr.peer`
//!   = attacker) and to every other player as `hitfx_in`; the receiving sidecars push it into
//!   the S2G ring as it is.
//! - [`DamageVerdict`]: the server's answers to a claim (`confirm`, `final`) and validated
//!   clashes (`hit_id` 0); the attacker's sidecar patches `cid` in place and pushes it S2G. Its
//!   local timeout / queue-full answers are the same record.
//! - [`DamageAck`]: the owner's sidecar acks every `damage_in` (C2S).
//! - [`ReplayOutcome`] / [`ReplayOutcomeAck`]: idempotent owner-native execution results,
//!   separate from transport receipt and the server's geometric verdict.
//! - [`Clash`] (`clash`, `touch`): parry / contact evidence, G2S then C2S as it is.
//! - [`DeathReport`] / [`DeathAck`]: the owner's own death (G2S from HSMPCombat and HSMPSync,
//!   resent C2S by the sidecar until acked); [`Death`]: a server-declared death (S2C; the
//!   server captures the original `match_id`, round and life and the sidecar pushes it S2G).
//! - [`Vitals`]: the owner's vitals, quantised once by the game (u16 = round(v · 64),
//!   0xFFFF unknown; dismembered parts as a bitmask over `dism_part`). Written into the game's
//!   `vitals` slot, gated and sent C2S by the sidecar (which patches `seq`), relayed S2C with
//!   `WireHdr.peer` = owner, written into that peer's `peer_vitals` slot.
//! - [`StandinDead`] (+ rows): game-local bus key `standin_dead` (HSMPCombat -> HSMPAvatars).

use super::{flow, Chan, Dir, EnumInfo, KindInfo, RecordInfo, SlotForm, SlotInfo, CAP_BUS, CAP_COMBAT, CAP_VITALS};
use crate::layout::{Bool, Str};
use crate::record::Invalid;

/// Legacy codec kinds of this domain: none (all moved to records).
pub const KINDS: &[KindInfo] = &[];

// ---- constants shared by the game, the sidecar and the server ------------------------------

/// Per-field damage deltas a claim may carry (HSMPCombat `FIELDS`, 0-based).
pub const MAX_DELTAS: usize = 24;
/// Valid delta field indices are `0..DELTA_FIELDS`.
pub const DELTA_FIELDS: u8 = 24;
/// Bone name capacity (bytes, `[A-Za-z0-9_]`).
pub const BONE_BYTES: usize = 32;
/// Claim flag bits (bit0 inside, bit1 lower_threshold, bit2 shockwave, bit3 stab, bit4 hit
/// flesh, bit5 armour-stage inputs, bit6 offset / normal / velocity / impulse in the hit
/// bone's frame, bit7 struck by a weapon rather than a body part).
pub const DAMAGE_FLAGS_ALL: u8 = 0xFF;
/// World coordinate bound (cm) for hit locations / offsets.
pub const LOC_LIMIT: f32 = 1.0e7;

/// Vitals scalars (`VITALS_NAMES` order, append only).
pub const VITALS_N: usize = 19;
/// Quantisation: `q = round(v · VITALS_SCALE)`, `VITALS_UNKNOWN` = unreadable.
pub const VITALS_SCALE: f32 = 64.0;
pub const VITALS_UNKNOWN: u16 = 0xFFFF;
/// Health-type scalars `0..=VITALS_LAST_HEALTH` are at most `VITALS_HP_CAP`; stamina and
/// exhaustion at most `VITALS_STAMINA_CAP` (the game clamps before quantising).
pub const VITALS_LAST_HEALTH: usize = 12;
pub const VITALS_HP_CAP: f32 = 150.0;
pub const VITALS_STAMINA_CAP: f32 = 200.0;
/// Vitals flag bits (Willie_BP_C booleans).
pub const VF_DEAD: u16 = 1;
pub const VF_FALLEN: u16 = 2;
pub const VF_DOWNED: u16 = 4;
pub const VF_HEADLESS: u16 = 8;
pub const VF_PAIN_SHOCK: u16 = 16;
// Current native functional injury state, not a sticky history of damage.
pub const VF_HEAD_IMPAIRED: u16 = 32;
pub const VF_NECK_IMPAIRED: u16 = 64;
pub const VF_BACK_IMPAIRED: u16 = 128;
pub const VF_ARM_R_IMPAIRED: u16 = 256;
pub const VF_ARM_L_IMPAIRED: u16 = 512;
pub const VF_LEG_R_IMPAIRED: u16 = 1024;
pub const VF_LEG_L_IMPAIRED: u16 = 2048;
pub const VF_ALL: u16 = 4095;

/// Reflected property names on Willie_BP_C, in `Vitals::v` order.
pub const VITALS_NAMES: [&str; VITALS_N] = [
    "Health", "Head Health", "Neck Health", "Body Upper Health", "Body Lower Health", "Back Health",
    "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health", "Head Health (Crush)",
    "Consciousness", "Consciousness 2 (Legs)", "Stamina", "Exhaustion", "Bleeding", "Blood Rate",
    "Pain", "Fallen Rate",
];

/// Dismemberable parts: bit `i` of `Vitals::dism` = `DISM_PARTS[i]` is severed (the Willie
/// bone names, posecodec v2 order).
pub const DISM_PARTS: [&str; 23] = [
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05", "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l", "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r",
];
pub const DISM_ALL: u32 = (1 << 23) - 1;

/// Server-declared death causes (`Death::cause`).
pub const DEATH_CAUSE_MAX: u8 = 4;

/// Stand-ins listed in one `standin_dead` value.
pub const STANDIN_DEAD_MAX: usize = 32;

// ---- records -------------------------------------------------------------------------------

crate::ipc_pod! {
    /// One per-field change the game's damage math made (HSMPCombat `FIELDS` index, signed).
    pub struct DamageDelta {
        pub i: u8,
        pub _r: [u8; 3],
        pub v: f32,
    }

    /// One damage claim (Half Sword `Deal Complex Damage` inputs on the attacker's stand-in of
    /// `target_peer_id`), and the same bytes as `damage_in` / `hitfx_in`. `n` delta rows follow.
    pub struct Damage {
        /// Attacker-sidecar hit id (unique per attacker connection; the game writes 0).
        pub hit_id: u32,
        /// The game's claim id (verdicts map back by it; 0 = none).
        pub cid: u32,
        /// The victim.
        pub target_peer_id: u32,
        /// Original server round captured by the game at native contact.
        pub round: u32,
        /// ms between the hit on the attacker's screen and this transmission (sidecar).
        pub age_ms: u32,
        /// ms between the hit and the game's send (game; the sidecar adds its own wait).
        pub lage_ms: u32,
        /// Lag compensation sender clocks, ms (0 = unknown): the attacker's own, the victim
        /// body and the victim arms the attacker was displaying.
        pub attacker_ts: u32,
        pub victim_view_ts: u32,
        pub victim_arm_ts: u32,
        /// Blunt Destruction Int | Kick·10 << 8 | Lower Threshold << 16 | Extra High << 17
        /// | native fist source << 18 | source left hand << 19 | source right hand << 20
        /// | native collision array ordinal (0 legacy, 1..15) << 21 | native foot source << 25
        /// | native DamageParent << 26 | distinct optional cutting HitBox ordinal << 27.
        pub dism_blunt: i32,
        pub raw_damage: f32,
        pub cutting_power: f32,
        pub pain_rate: f32,
        pub draw_cut: f32,
        pub damage_out: f32,
        /// Hit location minus the bone's world location (attacker's copy).
        pub offset: [f32; 3],
        /// World hit location on the attacker's screen.
        pub location: [f32; 3],
        pub impulse: [f32; 3],
        pub velocity: [f32; 3],
        pub normal: [f32; 3],
        /// Bone name, `[A-Za-z0-9_]`.
        pub bone: Str<32>,
        /// `DAMAGE_FLAGS_ALL` bits.
        pub flags: u8,
        /// Delta rows.
        pub n: u8,
        pub _r: [u8; 6],
        /// Captured authenticated match and both native pawn generations.
        pub match_id: u64,
        pub attacker_life: u16,
        pub victim_life: u16,
        pub _life_r: [u8; 4],
        /// Original native source class; modern component ordinals are class scoped.
        pub source_class: Str<48>,
        /// Cutting Box center in physical cm in the victim bone's rotation frame
        /// (no socket/reference-scale normalization), then its local quaternion, followed
        /// by original world scale and unscaled native BoxExtent. All zero without a Box.
        pub hit_box_frame: [f32; 13],
        pub _box_r: [u8; 4],
    }

    /// The server's answer to a claim, or a validated clash (`hit_id` 0, `peer` = the other
    /// player). `kind`: `verdict_kind`; `code`: `damage_reason`; `reason`: text for logs.
    pub struct DamageVerdict {
        pub hit_id: u32,
        /// The game's claim id (the attacker's sidecar fills it from its pending table).
        pub cid: u32,
        /// Clash: the other player.
        pub peer: u32,
        pub kind: u8,
        /// Final verdicts: accepted.
        pub ok: Bool,
        pub code: u8,
        pub _r: u8,
        pub reason: Str<48>,
    }

    /// The owner's sidecar got `damage_in` (`attacker`, `hit_id`) (C2S).
    pub struct DamageAck {
        pub attacker: u32,
        pub hit_id: u32,
    }

    /// Native owner attempt result, separate from the transport DamageAck.
    /// Context is the original approved hit, never assigned at receipt.
    pub struct ReplayOutcome {
        pub match_id: u64,
        pub round: u32,
        pub attacker: u32,
        pub hit_id: u32,
        pub victim_life: u16,
        pub status: u8,
        pub _r: u8,
        /// Bit i: native FIELDS[i] changed; this includes limbs/consciousness.
        pub observed_fields: u32,
        pub health_delta: f32,
    }

    /// `clash`: my weapon met `other_peer_id`'s; `touch`: its stand-in reached my body.
    /// `my_ts` = my sender clock, `other_ts` = the other's sender time I was displaying.
    pub struct Clash {
        pub other_peer_id: u32,
        pub my_ts: u32,
        pub other_ts: u32,
        pub _r: u32,
    }

    /// An original-life native death or final defeat. Only `death_id` is
    /// allocated by the sidecar; match, round, life and reason remain original.
    pub struct DeathReport {
        pub death_id: u32,
        pub round: u32,
        pub match_id: u64,
        pub life: u16,
        /// 0 = native death, 1 = Brawl final KO, 2 = deliberate surrender.
        pub reason: u8,
        pub _life_r: [u8; 5],
    }

    /// The server counted `death_id` (S2C).
    pub struct DeathAck {
        pub death_id: u32,
        pub _r: u32,
    }

    /// The server declared `peer_id` dead in `round` (S2C, re-sent while the round lasts;
    /// S2G once per (match, peer, round)). `cause`: 0 owner report, 1 damage ledger, 2 owner
    /// vitals, 3 left, 4 zone, 5 Brawl KO, 6 deliberate surrender.
    pub struct Death {
        pub peer_id: u32,
        pub round: u32,
        pub killer: u32,
        pub cause: u8,
        pub _r: u8,
        pub life: u16,
        /// The server's match id (the receiving sidecar fills it).
        pub match_id: u64,
        /// Wall clock ms at receipt (the receiving sidecar fills it).
        pub wall_ms: u64,
    }

    /// The owner's vitals, quantised at the source (see the module docs).
    pub struct Vitals {
        /// Writer's sequence (game: its own counter; on the wire: the sender sidecar's).
        pub seq: u32,
        /// Bit i = `DISM_PARTS[i]` severed.
        pub dism: u32,
        /// `VF_*` bits.
        pub flags: u16,
        /// `VITALS_NAMES` order: round(v · 64), `VITALS_UNKNOWN` = unknown.
        pub v: [u16; 19],
        pub match_id: u64,
        pub round: u32,
        pub life: u16,
        pub _life_r: [u8; 2],
    }

    /// Stand-ins whose death HSMPCombat is playing (bus key `standin_dead`).
    pub struct StandinDead {
        /// `os.time()` of the write (liveness).
        pub wall: u64,
        pub n: u16,
        pub _r: [u16; 3],
    }

    pub struct StandinDeadRow {
        pub peer: u32,
        pub _r: u32,
        /// The stand-in's actor FName.
        pub name: Str<56>,
    }
}

pub const K_DAMAGE: u16 = 0x0310;
pub const K_DAMAGE_IN: u16 = 0x0311;
pub const K_HITFX_IN: u16 = 0x0312;
pub const K_DAMAGE_VERDICT: u16 = 0x0313;
pub const K_DAMAGE_ACK: u16 = 0x0314;
pub const K_REPLAY_OUTCOME: u16 = 0x0315;
pub const K_REPLAY_OUTCOME_ACK: u16 = 0x0316;
pub const K_CLASH: u16 = 0x0318;
pub const K_TOUCH: u16 = 0x0319;
pub const K_DEATH_REPORT: u16 = 0x0320;
pub const K_DEATH_ACK: u16 = 0x0321;
pub const K_DEATH: u16 = 0x0322;
pub const K_VITALS: u16 = 0x0328;
pub const K_STANDIN_DEAD: u16 = 0x0330;

/// `DamageVerdict::kind`.
pub const VERDICT_CONFIRM: u8 = 1;
pub const VERDICT_FINAL: u8 = 2;
pub const VERDICT_CLASH: u8 = 3;

/// `DamageVerdict::code` (`damage_reason`): stable codes of the server's reasons (the text
/// before the first ':' of the reason, upper case) and the sidecar's own.
pub const DAMAGE_REASONS: &[(&str, u32)] = &[
    ("OK", 0), ("CONFIRM", 1), ("CLASH", 2), ("PARRIED", 3), ("TIMEOUT", 4), ("QUEUE_FULL", 5),
    ("EXPIRED", 6), ("ROUND_OVER", 7), ("ATTACKER_DOWN", 8), ("SELF_HIT", 9), ("NO_TARGET", 10),
    ("TARGET_DOWN", 11), ("NOT_LIVE", 12), ("STALE_ROUND", 13), ("TOO_LATE", 14), ("BAD_FIELD", 15),
    ("RANGE", 16), ("RATE_LIMITED", 17), ("NO_TS", 18), ("NO_HISTORY", 19), ("CLOCK_RATE", 20),
    ("TS_FUTURE", 21), ("TS_OLD", 22), ("TS_INCONSISTENT", 23), ("NO_CLOCK", 24), ("FUTURE", 25),
    ("REWIND_CAP", 26), ("BODY_MISS", 27), ("NO_COVER", 28), ("BLADE_MISS", 29), ("REACH", 30),
    ("OTHER", 31),
];
pub const REASON_OK: u8 = 0;
pub const REASON_CONFIRM: u8 = 1;
pub const REASON_CLASH: u8 = 2;
pub const REASON_TIMEOUT: u8 = 4;
pub const REASON_QUEUE_FULL: u8 = 5;
pub const REASON_OTHER: u8 = 31;

/// The `damage_reason` code of a reason text (`"code: details"`, any case; unknown = OTHER,
/// empty = OK).
pub fn reason_code_of(reason: &str) -> u8 {
    let c = reason.split(':').next().unwrap_or(reason).trim();
    if c.is_empty() {
        return REASON_OK;
    }
    // "round over / target down" (combat glue) has no colon.
    if c.starts_with("round over") {
        return 7;
    }
    DAMAGE_REASONS.iter().find(|(n, _)| n.eq_ignore_ascii_case(c)).map_or(REASON_OTHER, |(_, v)| *v as u8)
}

fn bone_ok(s: &Str<32>) -> bool {
    s.bytes().iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

fn v3_within(v: &[f32; 3], lim: f32) -> bool {
    v.iter().all(|c| c.abs() <= lim)
}

pub fn check_damage(d: &Damage) -> Result<(), Invalid> {
    // Armour-stage bits 0..17 are native damage inputs; 18 is a fist
    // pseudo-weapon, 19/20 identify left/right source hand. Old senders have
    // neither hand bit. Bits 21..24 are the native collision array ordinal
    // (1..15, 0 legacy). This identity cap differs from the eight pose boxes.
    if d.dism_blunt & (1 << 26) != 0 && d.flags & (1 << 5) == 0 {
        return Err(Invalid::Range("dism_blunt"));
    }
    if d.dism_blunt & 0x7be0_0000 != 0 && d.flags & ((1 << 5) | (1 << 7)) != ((1 << 5) | (1 << 7)) {
        return Err(Invalid::Range("dism_blunt"));
    }
    if d.flags & (1 << 5) != 0 && (d.dism_blunt < 0
        || (d.dism_blunt & 0x7800_0000 != 0 && (d.dism_blunt & 0x1e0_0000 == 0 || d.dism_blunt & 0x18_0000 == 0))
        || (d.dism_blunt & (1 << 25) != 0 && (d.dism_blunt & (1 << 18) != 0
            || d.dism_blunt & 0x18_0000 == 0 || d.dism_blunt & 0x1e0_0000 == 0))
        || d.dism_blunt & 0x18_0000 == 0x18_0000
        || (d.dism_blunt & 0x3fc_0000 != 0 && d.flags & (1 << 7) == 0)) {
        return Err(Invalid::Range("dism_blunt"));
    }
    let ordinal = (d.dism_blunt >> 21) & 15;
    let box_ordinal = (d.dism_blunt >> 27) & 15;
    if d.flags & (1<<7) != 0 && (d.flags & (1<<5) == 0 || ordinal==0) {return Err(Invalid::Range("dism_blunt"));}
    if ordinal != 0 && d.dism_blunt & 0x18_0000 == 0 { return Err(Invalid::Range("dism_blunt")); }
    if ordinal != 0 && d.source_class.is_empty() { return Err(Invalid::Range("source_class")); }
    if !d.source_class.as_str().is_some_and(|s|s.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'_')) {
        return Err(Invalid::Range("source_class"));
    }
    if d.hit_box_frame.iter().any(|v|!v.is_finite()) { return Err(Invalid::Float("hit_box_frame")); }
    if box_ordinal == 0 {
        if d.hit_box_frame.iter().any(|v| *v != 0.0) { return Err(Invalid::Range("hit_box_frame")); }
    } else {
        let a = &d.hit_box_frame;
        if d.flags & (1 << 6) == 0 || a[..3].iter().any(|v| v.abs() > 600.0)
            || !(0.99..=1.01).contains(&a[3..7].iter().map(|v|v*v).sum::<f32>())
            || a[7..10].iter().any(|v| *v < 1.0/1024.0 || *v >= 16.0)
            || a[10..13].iter().any(|v| *v <= 0.0 || *v > 300.0) {
            return Err(Invalid::Range("hit_box_frame"));
        }
    }
    if !bone_ok(&d.bone) {
        return Err(Invalid::Range("bone"));
    }
    // Every flag bit is defined (DAMAGE_FLAGS_ALL).
    if !v3_within(&d.location, LOC_LIMIT) {
        return Err(Invalid::Range("location"));
    }
    if !v3_within(&d.offset, LOC_LIMIT) {
        return Err(Invalid::Range("offset"));
    }
    if d.target_peer_id == 0 {
        return Err(Invalid::Range("target_peer_id"));
    }
    Ok(())
}

pub const REPLAY_CHANGED:u8=1;
pub const REPLAY_NO_OBSERVED_CHANGE:u8=2;
pub const REPLAY_STALE_CONTEXT:u8=3;
pub const REPLAY_SOURCE_MISSING:u8=4;
pub const REPLAY_INACTIVE:u8=5;
pub const REPLAY_UNCERTAIN:u8=6;
pub const REPLAY_EXPIRED:u8=7;
fn check_replay_outcome(r:&ReplayOutcome)->Result<(),Invalid> {
    if !(REPLAY_CHANGED..=REPLAY_EXPIRED).contains(&r.status) || r.attacker==0 || r.hit_id==0
        || r.observed_fields & !0x00ff_ffff != 0 || !r.health_delta.is_finite() {
        return Err(Invalid::Range("replay_outcome"));
    }
    if r.status!=REPLAY_CHANGED && (r.observed_fields!=0 || r.health_delta!=0.0) {
        return Err(Invalid::Range("replay_outcome"));
    }
    Ok(())
}

fn check_delta(_d: &Damage, r: &DamageDelta) -> Result<(), Invalid> {
    if r.i >= DELTA_FIELDS {
        return Err(Invalid::Range("i"));
    }
    Ok(())
}

fn check_verdict(v: &DamageVerdict) -> Result<(), Invalid> {
    if !(VERDICT_CONFIRM..=VERDICT_CLASH).contains(&v.kind) {
        return Err(Invalid::Range("kind"));
    }
    if v.code as usize >= DAMAGE_REASONS.len() {
        return Err(Invalid::Range("code"));
    }
    Ok(())
}

fn check_death(d: &Death) -> Result<(), Invalid> {
    if d.cause > DEATH_CAUSE_MAX {
        return Err(Invalid::Range("cause"));
    }
    if d.peer_id == 0 {
        return Err(Invalid::Range("peer_id"));
    }
    Ok(())
}

fn check_clash(c: &Clash) -> Result<(), Invalid> {
    if c.other_peer_id == 0 {
        return Err(Invalid::Range("other_peer_id"));
    }
    Ok(())
}

/// The largest quantised value scalar `i` may carry.
pub fn vitals_qmax(i: usize) -> u16 {
    let cap = if i <= VITALS_LAST_HEALTH { VITALS_HP_CAP } else if i <= 14 { VITALS_STAMINA_CAP } else { return VITALS_UNKNOWN - 1 };
    (cap * VITALS_SCALE).round() as u16
}

fn check_vitals(v: &Vitals) -> Result<(), Invalid> {
    if v.flags & !VF_ALL != 0 {
        return Err(Invalid::Range("flags"));
    }
    if v.dism & !DISM_ALL != 0 {
        return Err(Invalid::Range("dism"));
    }
    for (i, q) in v.v.iter().enumerate() {
        if *q != VITALS_UNKNOWN && *q > vitals_qmax(i) {
            return Err(Invalid::Range("v"));
        }
    }
    Ok(())
}

fn check_standin_row(_h: &StandinDead, r: &StandinDeadRow) -> Result<(), Invalid> {
    if r.peer == 0 {
        return Err(Invalid::Range("peer"));
    }
    Ok(())
}

crate::record!(Damage, kind = K_DAMAGE, name = "damage", rows = DamageDelta, count = n, max = MAX_DELTAS,
    check = check_damage, check_row = check_delta);
crate::record!(DamageVerdict, kind = K_DAMAGE_VERDICT, name = "damage_verdict", check = check_verdict);
crate::record!(DamageAck, kind = K_DAMAGE_ACK, name = "damage_ack");
crate::record!(ReplayOutcome, kind = K_REPLAY_OUTCOME, name = "replay_outcome", check = check_replay_outcome);
crate::record!(Clash, kind = K_CLASH, name = "clash", check = check_clash);
crate::record!(DeathReport, kind = K_DEATH_REPORT, name = "death_report");
crate::record!(DeathAck, kind = K_DEATH_ACK, name = "death_ack");
crate::record!(Death, kind = K_DEATH, name = "death", check = check_death);
crate::record!(Vitals, kind = K_VITALS, name = "vitals", check = check_vitals);
crate::record!(StandinDead, kind = K_STANDIN_DEAD, name = "standin_dead", rows = StandinDeadRow, count = n,
    max = STANDIN_DEAD_MAX, check_row = check_standin_row);

/// hsmp-net channel-0 stream of the vitals (`Latest` keyed by the owner).
pub const STREAM_VITALS: u8 = 8;

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[RecordInfo] = &[
    crate::record_info!(Damage, cap = CAP_COMBAT, flow = flow::G2S | flow::C2S, chan = Chan::Ordered,
        doc = "damage claim (game -> sidecar: hit_id / round / age_ms filled -> server)"),
    crate::record_info!(alias K_DAMAGE_IN, "damage_in", Damage, cap = CAP_COMBAT, flow = flow::S2C | flow::S2G,
        chan = Chan::Ordered, doc = "approved hit on the receiver's own pawn (peer = attacker)"),
    crate::record_info!(alias K_HITFX_IN, "hitfx_in", Damage, cap = CAP_COMBAT, flow = flow::S2C | flow::S2G,
        chan = Chan::Reliable, doc = "approved hit on another player (stand-in fx; peer = attacker)"),
    crate::record_info!(DamageVerdict, cap = CAP_COMBAT, flow = flow::S2C | flow::S2G, chan = Chan::Reliable,
        doc = "claim verdict (confirm / final) or validated clash; cid filled by the sidecar"),
    crate::record_info!(DamageAck, cap = CAP_COMBAT, flow = flow::C2S, chan = Chan::Reliable,
        doc = "owner sidecar acked a damage_in"),
    crate::record_info!(ReplayOutcome,cap=CAP_COMBAT,flow=flow::G2S|flow::C2S|flow::S2C|flow::S2G,chan=Chan::Reliable,
        doc="native owner replay result; receipt is independent of damage_ack"),
    crate::record_info!(alias K_REPLAY_OUTCOME_ACK,"replay_outcome_ack",ReplayOutcome,cap=CAP_COMBAT,flow=flow::S2C|flow::S2G,
        chan=Chan::Reliable,doc="server accepted the authenticated native owner attempt result"),
    crate::record_info!(Clash, cap = CAP_COMBAT, flow = flow::G2S | flow::C2S, chan = Chan::Reliable,
        doc = "weapon clash evidence (parry)"),
    crate::record_info!(alias K_TOUCH, "touch", Clash, cap = CAP_COMBAT, flow = flow::G2S | flow::C2S,
        chan = Chan::Reliable, doc = "a stand-in's blow reached my body (evidence against a parry)"),
    crate::record_info!(DeathReport, cap = CAP_COMBAT, flow = flow::G2S | flow::C2S, chan = Chan::Reliable,
        doc = "original-life native death or final defeat (resent until death_ack)"),
    crate::record_info!(DeathAck, cap = CAP_COMBAT, flow = flow::S2C, chan = Chan::Reliable,
        doc = "server counted a death_report"),
    crate::record_info!(Death, cap = CAP_COMBAT, flow = flow::S2C | flow::S2G, chan = Chan::Reliable,
        doc = "server-declared death (match_id / wall_ms filled by the sidecar)"),
    crate::record_info!(Vitals, cap = CAP_VITALS, flow = flow::G2S | flow::C2S | flow::S2C | flow::S2G,
        chan = Chan::Latest(STREAM_VITALS), doc = "owner vitals, quantised at the source (S2C peer = owner)"),
    crate::record_info!(StandinDead, cap = CAP_BUS, flow = flow::LOCAL, chan = Chan::None,
        doc = "stand-ins playing a death (HSMPCombat -> HSMPAvatars)"),
];

/// Named record slots of this domain.
pub const SLOTS: &[SlotInfo] = &[
    SlotInfo { name: "vitals", kind: K_VITALS, form: SlotForm::Slot, dir: Dir::GameToSidecar, cap: CAP_VITALS,
        world_scoped: true, doc: "the game's own vitals (HSMPCombat; read by the sidecar and the HUD)" },
    SlotInfo { name: "peer_vitals", kind: K_VITALS, form: SlotForm::PeerSlot, dir: Dir::SidecarToGame, cap: CAP_VITALS,
        world_scoped: false, doc: "a peer's latest vitals (highest seq)" },
    SlotInfo { name: "standin_dead", kind: K_STANDIN_DEAD, form: SlotForm::Bus, dir: Dir::Local, cap: CAP_BUS,
        world_scoped: true, doc: "stand-ins playing a death (HSMPCombat -> HSMPAvatars)" },
];

const DISM_ENUM: [(&str, u32); 23] = [
    ("PELVIS", 0), ("SPINE_01", 1), ("SPINE_02", 2), ("SPINE_03", 3), ("SPINE_04", 4), ("SPINE_05", 5),
    ("NECK_01", 6), ("NECK_02", 7), ("HEAD", 8), ("CLAVICLE_L", 9), ("UPPERARM_L", 10), ("LOWERARM_L", 11),
    ("HAND_L", 12), ("CLAVICLE_R", 13), ("UPPERARM_R", 14), ("LOWERARM_R", 15), ("HAND_R", 16),
    ("THIGH_L", 17), ("CALF_L", 18), ("FOOT_L", 19), ("THIGH_R", 20), ("CALF_R", 21), ("FOOT_R", 22),
];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[EnumInfo] = &[
    EnumInfo { name: "verdict_kind", values: &[("CONFIRM", VERDICT_CONFIRM as u32), ("FINAL", VERDICT_FINAL as u32), ("CLASH", VERDICT_CLASH as u32)] },
    EnumInfo {name:"replay_status",values:&[("CHANGED",1),("NO_OBSERVED_CHANGE",2),("STALE_CONTEXT",3),
        ("SOURCE_MISSING",4),("INACTIVE",5),("UNCERTAIN",6),("EXPIRED",7)]},
    EnumInfo { name: "damage_reason", values: DAMAGE_REASONS },
    // Bit index of each dismemberable part in `Vitals::dism`.
    EnumInfo { name: "dism_part", values: &DISM_ENUM },
    EnumInfo { name: "vitals_flag", values: &[("DEAD", VF_DEAD as u32), ("FALLEN", VF_FALLEN as u32), ("DOWNED", VF_DOWNED as u32),
        ("HEADLESS", VF_HEADLESS as u32), ("PAIN_SHOCK", VF_PAIN_SHOCK as u32),
        ("HEAD_IMPAIRED", VF_HEAD_IMPAIRED as u32), ("NECK_IMPAIRED", VF_NECK_IMPAIRED as u32),
        ("BACK_IMPAIRED", VF_BACK_IMPAIRED as u32), ("ARM_R_IMPAIRED", VF_ARM_R_IMPAIRED as u32),
        ("ARM_L_IMPAIRED", VF_ARM_L_IMPAIRED as u32), ("LEG_R_IMPAIRED", VF_LEG_R_IMPAIRED as u32),
        ("LEG_L_IMPAIRED", VF_LEG_L_IMPAIRED as u32)] },
    EnumInfo { name: "death_cause", values: &[("REPORTED", 0), ("DAMAGE", 1), ("VITALS", 2), ("LEFT", 3), ("ZONE", 4), ("DEFEAT", 5), ("SURRENDER", 6)] },
    EnumInfo { name: "death_report_reason", values: &[("DEATH", 0), ("DEFEAT", 1), ("SURRENDER", 2)] },
];

// ---- helpers (writers and readers in Rust) --------------------------------------------------

/// Quantise one vitals scalar (non-finite -> unknown; clamped to the field's range).
pub fn vitals_q(i: usize, x: f32) -> u16 {
    if !x.is_finite() {
        return VITALS_UNKNOWN;
    }
    let max = vitals_qmax(i);
    ((x.max(0.0) * VITALS_SCALE).round().min(max as f32)) as u16
}

/// A quantised vitals scalar as a value (`None` = unknown).
pub fn vitals_dq(q: u16) -> Option<f32> {
    (q != VITALS_UNKNOWN).then(|| q as f32 / VITALS_SCALE)
}

/// The `dism` bit of a part name (any case), if it is a known part.
pub fn dism_bit(name: &str) -> Option<u32> {
    DISM_PARTS.iter().position(|p| p.eq_ignore_ascii_case(name)).map(|i| 1u32 << i)
}

impl Vitals {
    pub fn health(&self) -> Option<f32> {
        vitals_dq(self.v[0])
    }
    pub fn dead(&self) -> bool {
        self.flags & VF_DEAD != 0 || self.health().is_some_and(|h| h <= 0.0)
    }
    /// The severed part names.
    pub fn dism_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        DISM_PARTS.iter().enumerate().filter(move |(i, _)| self.dism & (1 << i) != 0).map(|(_, n)| *n)
    }
}

impl Damage {
    /// The bone as text (validated records are always UTF-8).
    pub fn bone_str(&self) -> &str {
        self.bone.as_str().unwrap_or("")
    }
}

impl DamageDelta {
    pub fn new(i: u8, v: f32) -> Self {
        DamageDelta { i, _r: [0; 3], v }
    }
}

impl DamageVerdict {
    /// A verdict record (`reason` truncated to its capacity).
    pub fn new(hit_id: u32, cid: u32, kind: u8, ok: bool, reason: &str) -> Self {
        DamageVerdict {
            hit_id,
            cid,
            peer: 0,
            kind,
            ok: Bool::from(ok),
            code: if kind == VERDICT_CONFIRM { REASON_CONFIRM } else if kind == VERDICT_CLASH { REASON_CLASH } else { reason_code_of(reason) },
            _r: 0,
            reason: Str::new(reason),
        }
    }
}

impl Death {
    pub fn new(peer_id: u32, round: u32, killer: u32, cause: u8) -> Self {
        Death { peer_id, round, killer, cause, _r: 0, life: 0, match_id: 0, wall_ms: 0 }
    }
}

impl StandinDeadRow {
    pub fn new(peer: u32, name: &str) -> Self {
        StandinDeadRow { peer, _r: 0, name: Str::new(name) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{to_payload, view};

    fn claim() -> Damage {
        Damage {
            hit_id: 7, cid: 3, target_peer_id: 2, round: 1, age_ms: 9, lage_ms: 4, attacker_ts: 100,
            victim_view_ts: 90, victim_arm_ts: 95, dism_blunt: 2561, raw_damage: 5.0, cutting_power: 6.0,
            pain_rate: 0.5, draw_cut: 0.0, damage_out: 0.8, offset: [1.0; 3], location: [2.0; 3],
            impulse: [3.0; 3], velocity: [4.0; 3], normal: [0.0, 0.0, 1.0], bone: Str::new("neck_01"),
            flags: 32, n: 0, _r: [0; 6], ..Default::default()
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(core::mem::size_of::<Damage>(), 280);
        assert_eq!(core::mem::size_of::<DamageDelta>(), 8);
        assert_eq!(core::mem::size_of::<DamageVerdict>(), 64);
        assert_eq!(core::mem::size_of::<Vitals>(), 64);
        assert_eq!(core::mem::size_of::<Death>(), 32);
        assert_eq!(core::mem::size_of::<Clash>(), 16);
        assert!(crate::record::payload_len::<Damage>(MAX_DELTAS) <= crate::ring::MAX_PAYLOAD);
    }

    #[test]
    fn damage_round_trip_and_checks() {
        let rows = [DamageDelta::new(0, -35.25), DamageDelta::new(11, 4.0)];
        let p = to_payload(&claim(), &rows);
        let v = view::<Damage>(&p).unwrap();
        assert_eq!(v.head.n, 2);
        assert_eq!(&*v.rows, &rows);
        assert_eq!(v.head.bone, "neck_01");
        // Every range check.
        let bad = |f: &dyn Fn(&mut Damage)| {
            let mut d = claim();
            f(&mut d);
            view::<Damage>(&to_payload(&d, &[])).unwrap_err()
        };
        assert_eq!(bad(&|d| d.bone = Str::new("neck\"}")), Invalid::Range("bone"));
        let mut all = claim();
        all.flags = DAMAGE_FLAGS_ALL;
        all.dism_blunt=(1<<20)|(1<<21);all.source_class=Str::new("ModularWeaponBP_ArmingSword_C");
        assert!(view::<Damage>(&to_payload(&all, &[])).is_ok(), "every flag bit is defined");
        assert_eq!(bad(&|d| d.location[1] = 2.0e7), Invalid::Range("location"));
        assert_eq!(bad(&|d| d.offset[0] = -2.0e7), Invalid::Range("offset"));
        assert_eq!(bad(&|d| d.target_peer_id = 0), Invalid::Range("target_peer_id"));
        assert_eq!(bad(&|d| d.velocity[2] = f32::NAN), Invalid::Float("velocity"));
        assert_eq!(bad(&|d| d.raw_damage = f32::INFINITY), Invalid::Float("raw_damage"));
        for source in [1 << 18, 1 << 19, 1 << 20, (1 << 18) | (1 << 19)] {
            let mut d = claim(); d.flags |= 128; d.dism_blunt |= source;
            assert!(view::<Damage>(&to_payload(&d, &[])).is_err(), "protocol 8 rejects unbound ordinal-zero weapon claims");
        }
        assert_eq!(bad(&|d| { d.flags |= 128; d.dism_blunt |= (1 << 19) | (1 << 20); }), Invalid::Range("dism_blunt"));
        for ordinal in 1..=15 {
            let mut d = claim(); d.flags |= 128; d.dism_blunt |= (1<<20) | ordinal << 21;
            d.source_class=Str::new("BP_Longsword_Tier3_C");
            assert!(view::<Damage>(&to_payload(&d, &[])).is_ok());
        }
        assert_eq!(bad(&|d| { d.flags |= 128; d.dism_blunt |= 1 << 27; }), Invalid::Range("dism_blunt"));
        for ordinal in 1..=15 {
            let mut d = claim(); d.flags |= 128|64;
            d.dism_blunt |= (1 << 19) | (3 << 21) | (ordinal << 27);
            d.source_class=Str::new("BP_Longsword_Tier3_C");
            d.hit_box_frame=[0.0,0.0,0.0,0.0,0.0,0.0,1.0,2.0,1.0,1.0,10.0,2.0,20.0];
            assert!(view::<Damage>(&to_payload(&d, &[])).is_ok(), "distinct native cutting box {ordinal}");
        }
        assert_eq!(bad(&|d| { d.flags |= 128; d.dism_blunt |= (3 << 21) | (1 << 27); }), Invalid::Range("dism_blunt"));
        assert_eq!(bad(&|d| { d.flags &= !32; d.flags |= 128; d.dism_blunt |= (1 << 19) | (3 << 21) | (1 << 27); }), Invalid::Range("dism_blunt"));
        assert_eq!(bad(&|d| { d.dism_blunt = i32::MIN; }), Invalid::Range("dism_blunt"));
        let mut body_parent = claim(); body_parent.dism_blunt |= 1 << 26;
        assert!(view::<Damage>(&to_payload(&body_parent, &[])).is_ok());
        assert_eq!(bad(&|d| { d.flags &= !32; d.dism_blunt |= 1 << 26; }), Invalid::Range("dism_blunt"));
        let mut foot = claim(); foot.flags |= 128; foot.dism_blunt |= (1 << 25) | (1 << 19) | (10 << 21);
        foot.source_class=Str::new("Weapon_Feet_C");
        assert!(view::<Damage>(&to_payload(&foot, &[])).is_ok());
        assert_eq!(bad(&|d| { d.flags |= 128; d.dism_blunt |= (1 << 25) | (1 << 18) | (1 << 19) | (10 << 21); }), Invalid::Range("dism_blunt"));
        assert_eq!(bad(&|d| { d.flags |= 128; d.dism_blunt |= (1 << 25) | (10 << 21); }), Invalid::Range("dism_blunt"));
        assert_eq!(bad(&|d| { d.flags |= 128; d.flags &= !32; d.dism_blunt |= 1 << 21; }), Invalid::Range("dism_blunt"));
        assert_eq!(bad(&|d| d.dism_blunt |= 1 << 18), Invalid::Range("dism_blunt"));
        let p = to_payload(&claim(), &[DamageDelta::new(24, 1.0)]);
        assert_eq!(view::<Damage>(&p).unwrap_err(), Invalid::Range("i"));
        let p = to_payload(&claim(), &[DamageDelta::new(1, f32::NAN)]);
        assert_eq!(view::<Damage>(&p).unwrap_err(), Invalid::Float("v"));
        // Hostile counts / sizes.
        let mut h = claim();
        h.n = 25;
        let mut p = bytemuck::bytes_of(&h).to_vec();
        p.extend(std::iter::repeat_n(0u8, 25 * 8));
        assert!(matches!(view::<Damage>(&p), Err(Invalid::Rows { .. })));
        let p = to_payload(&claim(), &rows);
        assert!(view::<Damage>(&p[..p.len() - 1]).is_err());
        // An empty bone is a server decision (verdict), not a malformed record.
        let mut e = claim();
        e.bone = Str::new("");
        assert!(view::<Damage>(&to_payload(&e, &[])).is_ok());
    }

    #[test]
    fn verdict_death_clash_checks() {
        let v = DamageVerdict::new(5, 0, VERDICT_FINAL, false, "range: out of range (900 > 800)");
        assert_eq!(v.code, 16);
        assert_eq!(v.reason, "range: out of range (900 > 800)");
        assert!(view::<DamageVerdict>(&to_payload(&v, &[])).is_ok());
        let mut b = v;
        b.kind = 0;
        assert_eq!(view::<DamageVerdict>(&to_payload(&b, &[])).unwrap_err(), Invalid::Range("kind"));
        b = v;
        b.code = 200;
        assert_eq!(view::<DamageVerdict>(&to_payload(&b, &[])).unwrap_err(), Invalid::Range("code"));
        let mut p = to_payload(&v, &[]);
        p[13] = 2; // ok bool
        assert_eq!(view::<DamageVerdict>(&p).unwrap_err(), Invalid::Bool("ok"));
        assert_eq!(reason_code_of(""), REASON_OK);
        assert_eq!(reason_code_of("parried"), 3);
        assert_eq!(reason_code_of("Blade_Miss: x"), 29);
        assert_eq!(reason_code_of("round over / target down"), 7);
        assert_eq!(reason_code_of("weird: x"), REASON_OTHER);
        assert_eq!(DamageVerdict::new(1, 2, VERDICT_CONFIRM, true, "confirm").code, REASON_CONFIRM);

        let d = Death::new(2, 1, 3, 1);
        assert!(view::<Death>(&to_payload(&d, &[])).is_ok());
        let mut x = d;
        x.cause = 5;
        assert_eq!(view::<Death>(&to_payload(&x, &[])).unwrap_err(), Invalid::Range("cause"));
        x = d;
        x.peer_id = 0;
        assert_eq!(view::<Death>(&to_payload(&x, &[])).unwrap_err(), Invalid::Range("peer_id"));

        let c = Clash { other_peer_id: 4, my_ts: 10, other_ts: 20, _r: 0 };
        assert!(view::<Clash>(&to_payload(&c, &[])).is_ok());
        assert_eq!(view::<Clash>(&to_payload(&Clash { other_peer_id: 0, ..c }, &[])).unwrap_err(), Invalid::Range("other_peer_id"));
        assert!(view::<DeathReport>(&to_payload(&DeathReport { death_id: 1, round: 2, ..Default::default() }, &[])).is_ok());
        assert!(view::<DamageAck>(&[0u8; 7]).is_err());
    }

    #[test]
    fn vitals_quantise_and_checks() {
        let mut v = Vitals { seq: 1, dism: 0, flags: VF_FALLEN, v: [VITALS_UNKNOWN; 19], ..Default::default() };
        v.v[0] = vitals_q(0, 87.5);
        v.v[13] = vitals_q(13, 61.5);
        assert_eq!(v.health(), Some(87.5));
        assert_eq!(vitals_dq(v.v[13]), Some(61.5));
        assert_eq!(vitals_q(0, 1e9), vitals_qmax(0));
        assert_eq!(vitals_q(0, -3.0), 0);
        assert_eq!(vitals_q(1, f32::NAN), VITALS_UNKNOWN);
        assert!(!v.dead());
        v.dism = dism_bit("LowerArm_L").unwrap() | dism_bit("hand_l").unwrap();
        assert_eq!(v.dism_names().collect::<Vec<_>>(), vec!["lowerarm_l", "hand_l"]);
        assert!(dism_bit("tail").is_none());
        assert!(view::<Vitals>(&to_payload(&v, &[])).is_ok());
        let mut b = v;
        b.flags = VF_ALL;
        assert!(view::<Vitals>(&to_payload(&b, &[])).is_ok(), "current native regional injuries fit the existing flags field");
        b.flags = 4096;
        assert_eq!(view::<Vitals>(&to_payload(&b, &[])).unwrap_err(), Invalid::Range("flags"));
        b = v;
        b.dism = 1 << 23;
        assert_eq!(view::<Vitals>(&to_payload(&b, &[])).unwrap_err(), Invalid::Range("dism"));
        b = v;
        b.v[0] = vitals_qmax(0) + 1;
        assert_eq!(view::<Vitals>(&to_payload(&b, &[])).unwrap_err(), Invalid::Range("v"));
        b = v;
        b.v[18] = 0xFFFE;
        assert!(view::<Vitals>(&to_payload(&b, &[])).is_ok(), "fallen rate up to 1023.97");
        b.v[0] = 0;
        assert!(b.dead());
    }

    #[test]
    fn standin_dead_rows() {
        let h = StandinDead { wall: 1, n: 0, _r: [0; 3] };
        let rows = [StandinDeadRow::new(2, "Willie_BP_C_2147481234")];
        let p = to_payload(&h, &rows);
        let v = view::<StandinDead>(&p).unwrap();
        assert_eq!(v.rows[0].name, "Willie_BP_C_2147481234");
        assert_eq!(view::<StandinDead>(&to_payload(&h, &[StandinDeadRow::new(0, "x")])).unwrap_err(), Invalid::Range("peer"));
    }

    #[test]
    fn native_outcome_schema_separates_execution_from_delivery() {
        let r=ReplayOutcome{match_id:5,round:2,attacker:7,hit_id:9,victim_life:3,status:REPLAY_CHANGED,
            observed_fields:1,health_delta:-5.0,..Default::default()};
        assert!(view::<ReplayOutcome>(&to_payload(&r,&[])).is_ok());
        for status in [REPLAY_SOURCE_MISSING,REPLAY_UNCERTAIN,REPLAY_EXPIRED] {
            let mut b=r;b.status=status;
            assert!(view::<ReplayOutcome>(&to_payload(&b,&[])).is_err(),"nonexecuted/uncertain result cannot claim observed injury");
            b.observed_fields=0;b.health_delta=0.0;
            assert!(view::<ReplayOutcome>(&to_payload(&b,&[])).is_ok());
        }
        let mut b=r;b.observed_fields=1<<24;
        assert!(view::<ReplayOutcome>(&to_payload(&b,&[])).is_err());
        b=r;b.health_delta=f32::NAN;
        assert!(view::<ReplayOutcome>(&to_payload(&b,&[])).is_err());
        for kind in [K_REPLAY_OUTCOME,K_REPLAY_OUTCOME_ACK] {
            assert_eq!(crate::schema::record_info(kind).unwrap().chan,Chan::Reliable);
        }
    }

    #[test]
    fn every_kind_has_a_record() {
        for k in [K_DAMAGE, K_DAMAGE_IN, K_HITFX_IN, K_DAMAGE_VERDICT, K_DAMAGE_ACK, K_CLASH, K_TOUCH, K_DEATH_REPORT,
                  K_DEATH_ACK, K_DEATH, K_VITALS, K_STANDIN_DEAD] {
            assert!(crate::schema::record_info(k).is_some(), "{k:#06x}");
        }
        assert_eq!(VITALS_NAMES.len(), VITALS_N);
    }

    #[test]
    fn native_damage_routes_are_ordered_without_ordering_bulk_downloads() {
        for kind in [K_DAMAGE, K_DAMAGE_IN] {
            assert_eq!(crate::schema::record_info(kind).unwrap().chan, Chan::Ordered);
        }
        for kind in [K_HITFX_IN, K_DAMAGE_VERDICT, K_DAMAGE_ACK] {
            assert_eq!(crate::schema::record_info(kind).unwrap().chan, Chan::Reliable);
        }
        assert_eq!(crate::schema::record_by_name("mod_chunk").unwrap().chan, Chan::Reliable,
            "20 MiB mod data must not block the ordered native combat stream");
    }
}
