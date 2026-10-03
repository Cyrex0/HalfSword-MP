//! Damage plausibility: a PRINCIPLED bound
//! derived from Half Sword's own damage Blueprints (Willie_BP "Get Damage" /
//! "Deal Complex Damage", ModularWeaponBP "Collision Hit"; decompiled, see
//! docs/development/subsystems/combat.md):
//!
//! ```text
//!   cap(class, speed, armour, part) = MERGE_HEADROOM ×
//!       2·hm_max · Raw_max · kH[part] · 0.00025      (0 below the DRS gate)
//!   Raw_max = through_armour(rig_max(class)·1.1·2.333, speed·SPEED_TOL·hit_vel_factor(class))
//! ```
//!
//! The attacker-reported Health loss (its `deltas` / `damage_out`) is a
//! claim: anything above the cap is scaled down (`clamp_hit`) before the hit
//! is booked by the ledger or forwarded to the owner, and counted in
//! `validate::cheat`. Every unknown (blade alignment, weapon quality, normal
//! impulse, Willie height) is taken at its worst case, so an honest hit is
//! never clamped unless the server under-measured its speed; the factors
//! that are not read straight from the game data (hit_vel_factor,
//! SPEED_TOL, MERGE_HEADROOM) are marked UNCALIBRATED: check the
//! `combat: damage clamped` log lines of honest fights.
//!
//! Kits: `set_kit` (glue in loadout.rs after a kit resolves) tells the model
//! which weapons each player holds and what armour covers which bones. With
//! no kit (free mode, "none" class, glue missing) the cap is the most
//! permissive one: unknown weapon, no armour.

use crate::loadout::KitSel;
use crate::proto::{DamageEvent, PeerId};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Index of `Health` in the shared HSMPCombat FIELDS table.
pub const FIELD_HEALTH: u8 = 0;
/// Hard per-hit ceiling (Willie Health is 100; one blow may kill).
pub const LOSS_CEILING: f32 = 100.0;
/// Below this cap nothing is ever clamped (bruises, scratches).
pub const LOSS_FLOOR: f32 = 8.0;
/// Claims above `SEVERE × cap` count as severe in the cheat score.
pub const SEVERE: f32 = 1.5;
/// Hit flags (proto DamageEvent::flags).
pub const FLAG_STAB: u8 = 1 << 3;
pub const FLAG_INSIDE: u8 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponClass {
    /// Kit unknown: most permissive.
    Unknown,
    Unarmed,
    Dagger,
    Sword,
    Axe,
    Blunt,
    Polearm,
    Shield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Armour {
    None,
    Padded,
    Mail,
    Plate,
}

/// Weapon class from a catalogue item id (`loadout::catalog`).
pub fn class_of(item_id: &str) -> WeaponClass {
    let id = item_id.to_ascii_lowercase();
    let has = |k: &[&str]| k.iter().any(|s| id.contains(s));
    let group = crate::loadout::catalog::item(item_id).map(|i| i.group);
    if id.is_empty() {
        WeaponClass::Unarmed
    } else if group == Some("shield") || has(&["shield", "buckler", "pavise", "targe", "_lid", "bossgrip"]) {
        WeaponClass::Shield
    } else if group == Some("dagger") || has(&["dagger", "rondel", "knife", "chisel", "scissors"]) {
        WeaponClass::Dagger
    } else if has(&["spear", "pitchfork", "halberd", "billhook", "poleaxe", "scythe", "hoe", "rake"]) {
        WeaponClass::Polearm
    } else if has(&["axe", "hatchet", "pickaxe", "sickle"]) {
        WeaponClass::Axe
    } else if has(&["mace", "hammer", "flail", "staff", "mallet", "shovel", "tongs", "maul", "stool", "candle", "lantern"]) {
        WeaponClass::Blunt
    } else if has(&["sword", "arming", "falchion", "bastard", "messer"]) {
        WeaponClass::Sword
    } else {
        WeaponClass::Unknown
    }
}

/// Armour layer of an armour item id.
pub fn layer_of(item_id: &str) -> Armour {
    let id = item_id.to_ascii_lowercase();
    let has = |k: &[&str]| k.iter().any(|s| id.contains(s));
    if has(&["h_hat", "h_cap", "b_shirt", "b_tunic", "b_doublet", "l_hosen", "l_trousers", "f_shoes", "t_tabard"]) {
        Armour::None
    } else if has(&["gambeson", "jack", "b_arming"]) {
        Armour::Padded
    } else if has(&["hauberk", "mail", "n_standart"]) {
        Armour::Mail
    } else {
        // Kettle hats, sallets, bevors, spaulders, vambraces, gauntlets,
        // breastplates, faulds, cuisses, greaves: plate.
        Armour::Plate
    }
}

/// Armour slot groups (loadout catalogue) covering a hit bone.
pub fn groups_for_bone(bone: &str) -> &'static [&'static str] {
    let b = bone.to_ascii_lowercase();
    if b.starts_with("head") || b.contains("neck") {
        &["head", "bevor", "collar"]
    } else if b.starts_with("upperarm") || b.contains("clavicle") {
        &["shoulders", "arms", "mail", "body"]
    } else if b.starts_with("lowerarm") {
        &["arms", "body"]
    } else if b.starts_with("hand") {
        &["hands"]
    } else if b.starts_with("thigh") {
        &["thighs", "waist", "mail"]
    } else if b.starts_with("calf") {
        &["shins"]
    } else if b.starts_with("foot") || b.starts_with("ball") {
        &["feet"]
    } else if b.starts_with("pelvis") {
        &["waist", "mail", "body", "chest"]
    } else {
        // spine_*, torso.
        &["chest", "mail", "body"]
    }
}

// ---- the game's own damage math (Willie_BP "Get Damage" / "Deal Complex
// Damage", ModularWeaponBP "Collision Hit"; decompiled, see
// docs/development/subsystems/combat.md) ---------------------------------
//
//   Raw    = R_eff · |HitVel|                       (unarmoured)
//   R_eff  = rig_tag · quality(≤ 1.1) · (1 + (EdgeAlign + TipAlign)(M − 1)),
//            M ≤ 1.666 (blunt contact, cutting rate 0) → ≤ 2.333 · 1.1 · rig
//   HitVel = max(|weapon COM velocity| cm/s, |normal impulse| kg·cm/s)
//   armour (layers summed over the bodies traced before flesh):
//            E = CP·V·R, resist = (DefCut·(1−s) + DefStab·s)·1000,
//            frac = max(0, E − resist)/E,
//            Raw = max(0, R·V·(1−frac) − 100·DefBlunt) + R·V·frac
//   DRS    = 2 · hm(≤ 1.125) · Raw · GI damage rate (default 1)
//   Health −= DRS · kH[part] · 0.00025   only if DRS > 1000 (or Inside)
//            kH = head 15, neck 7.5, upper torso 5, lower torso 2.5, limbs 0.1
//   Snap Neck (Health = 0) once Neck Health ≤ 25 and the hit is strong.
//
// The server only knows the striking part's speed (lagcomp history) and
// the kits, so every unknown is taken at its worst case: best quality,
// blunt alignment (×2.333), the heaviest weapon of the class (rig tag), the
// normal impulse bounded by HIT_VEL_FACTOR × speed, the tallest Willie
// (hm 1.125). A claim above that bound is physically impossible.

/// Largest `rigNNN` tag per class in shipped content (weapon_tags /
/// module_tags): sword & falchion blades 0.70–0.85, rondels 1.00, axe heads
/// ≤ 1.25, maces 1.33 / hammers 1.10 / flail heads 2.00, poleaxe & hafted
/// heads ≤ 1.45, shields ≤ 1.00, traps 2.50. Unarmed: Willie-vs-Willie body
/// contact (Deal Complex Damage with the body's own rigidity, assumed 1).
pub fn rig_max(c: WeaponClass) -> f32 {
    match c {
        WeaponClass::Unknown => 2.50,
        WeaponClass::Unarmed => 1.00,
        WeaponClass::Dagger => 1.00,
        WeaponClass::Sword => 0.85,
        WeaponClass::Axe => 1.25,
        WeaponClass::Blunt => 2.00,
        WeaponClass::Polearm => 1.45,
        WeaponClass::Shield => 1.00,
    }
}
/// Best-quality rigidity factor (Quality 0 → ×1.1).
pub const QUALITY_MAX: f32 = 1.1;
/// Alignment amplification at a fully aligned blunt contact: 1 + 2·0.666.
pub const ALIGN_MAX: f32 = 2.333;
/// |normal impulse| / |striking-part speed| bound per class: the game uses
/// the larger of the weapon's COM velocity and the collision's normal
/// impulse (kg·cm/s); impulse ≈ reduced mass × (1 + e) × Δv, so heavier
/// weapons can exceed their speed. Reduced mass vs a ~5 kg body segment:
/// dagger 0.4 kg, sword 1.2–1.5, axes / maces / polearms 2–3, an arm 3–4.
/// UNCALIBRATED (in-game log `combat: damage clamped` ratios).
pub fn hit_vel_factor(c: WeaponClass) -> f32 {
    match c {
        WeaponClass::Dagger => 1.2,
        WeaponClass::Sword => 1.8,
        WeaponClass::Axe | WeaponClass::Blunt | WeaponClass::Polearm | WeaponClass::Shield => 2.5,
        WeaponClass::Unarmed | WeaponClass::Unknown => 3.0,
    }
}
/// Tallest / strongest Willie (hm = MapRange(Height + Muscle, 0..2 → 1.125..0.875)).
pub const HM_MAX: f32 = 1.125;
/// GI "Player Damage Rate" assumed (difficulty setting; 1 = default).
pub const DAMAGE_RATE_MAX: f32 = 1.0;
/// Health loss happens only above this DRS (or for Inside / stab contacts).
pub const DRS_GATE: f32 = 1000.0;
/// One HSMPCombat claim carries a TICK's Get Damage calls on one stand-in
/// (the blade crossing two bodies in the same frame both deal damage; the
/// claim names the most damaging part).
pub const MERGE_HEADROOM: f32 = 1.5;
/// Below this cap nothing is ever clamped: bleeding drain over the 33 ms
/// measurement tick, scratches.
pub const LOSS_FLOOR_MEASURE: f32 = 0.5;

/// Get Damage body parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part { Head, Neck, ArmR, ArmL, UpperTorso, LowerTorso, LegR, LegL }

/// `kH` (Health factor per part) from Get Damage.
pub fn k_health(p: Part) -> f32 {
    match p {
        Part::Head => 15.0,
        Part::Neck => 7.5,
        Part::UpperTorso => 5.0,
        Part::LowerTorso => 2.5,
        Part::ArmR | Part::ArmL | Part::LegR | Part::LegL => 0.1,
    }
}

/// Body part of a hit bone (a bone in no list counts as Upper torso, as in
/// the game).
pub fn part_of(bone: &str) -> Part {
    let b = bone.to_ascii_lowercase();
    let r = b.ends_with("_r");
    if b.starts_with("head") || b.contains("jaw") || b.contains("eye") {
        Part::Head
    } else if b.contains("neck") {
        Part::Neck
    } else if b.starts_with("upperarm") || b.starts_with("lowerarm") || b.starts_with("hand")
        || b.contains("thumb") || b.contains("index") || b.contains("middle") || b.contains("ring") || b.contains("pinky") {
        if r { Part::ArmR } else { Part::ArmL }
    } else if b.starts_with("thigh") || b.starts_with("calf") || b.starts_with("foot") || b.starts_with("ball") {
        if r { Part::LegR } else { Part::LegL }
    } else if b.starts_with("pelvis") || b.starts_with("spine_01") || b.starts_with("spine_02") {
        Part::LowerTorso
    } else {
        Part::UpperTorso
    }
}

/// Armour protection of a layer (Protection Blunt / Cut / Stab of the
/// weakest common piece of that layer in shipped content).
pub fn protection(a: Armour) -> (f32, f32, f32) {
    match a {
        Armour::None => (0.0, 0.0, 0.0),
        // Gambeson 5 / 20 / 2 (jack 7.5 / 25 / 2.5).
        Armour::Padded => (5.0, 20.0, 2.0),
        // Hauberk / mail faulds 1 / 150 / 15.
        Armour::Mail => (1.0, 150.0, 15.0),
        // Gauntlets 10 / 200 / 75 are the weakest plate.
        Armour::Plate => (10.0, 200.0, 75.0),
    }
}

/// Raw damage bound after one armour layer (Deal Complex Damage), worst
/// case over the cutting power (≤ 200) and the stab fraction s ∈ {0, 1}.
fn raw_through(r: f32, v: f32, a: Armour) -> f32 {
    let (db, dc, ds) = protection(a);
    if db == 0.0 && dc == 0.0 { return r * v; }
    let mut best = 0.0f32;
    for s in [0.0f32, 1.0] {
        let e = 200.0 * v * r;
        let resist = (dc * (1.0 - s) + ds * s) * 1000.0;
        let frac = if e > 0.0 { ((e - resist) / e).max(0.0) } else { 0.0 };
        let raw = (r * v * (1.0 - frac) - 100.0 * db).max(0.0) + r * v * frac;
        best = best.max(raw);
    }
    best
}

/// Upper bound of the game's Health loss for ONE Get Damage call of `class`
/// at striking speed `speed` (uu/s = cm/s; ∞ = unknown) on `part` under
/// `armour`. `stab`: Inside / stab contacts skip the DRS gate.
pub fn native_bound_part(class: WeaponClass, speed: f32, armour: Armour, stab: bool, part: Part) -> f32 {
    if !speed.is_finite() { return LOSS_CEILING; }
    let r = rig_max(class) * QUALITY_MAX * ALIGN_MAX;
    let v = speed.max(0.0) * hit_vel_factor(class);
    let drs = 2.0 * HM_MAX * DAMAGE_RATE_MAX * raw_through(r, v, armour);
    if drs <= DRS_GATE && !stab { return 0.0; }
    (drs * k_health(part) * 0.00025).min(LOSS_CEILING)
}

/// `native_bound_part` for a bone name.
pub fn native_bound(class: WeaponClass, speed: f32, armour: Armour, stab: bool, bone: &str) -> f32 {
    native_bound_part(class, speed, armour, stab, part_of(bone))
}

/// Server cap for one claim: the native bound at the server-measured speed
/// (× SPEED_TOL for the 60 Hz history), × MERGE_HEADROOM, floored and
/// clamped. `bone` "" = head (most permissive part).
pub fn loss_cap_bone(class: WeaponClass, speed: Option<f32>, armour: Armour, stab: bool, bone: &str) -> f32 {
    let v = match speed { Some(v) if v.is_finite() => v * SPEED_TOL, _ => f32::INFINITY };
    let part = if bone.is_empty() { Part::Head } else { part_of(bone) };
    (native_bound_part(class, v, armour, stab, part) * MERGE_HEADROOM).clamp(LOSS_FLOOR_MEASURE, LOSS_CEILING)
}

/// Server-measured striking speed vs the physics velocity the game used.
/// 1.0: lag comp already takes the MAX over the last two frames before the
/// contact (`SPEED_FRAMES`), which over-reads a decelerating blade by up to
/// ~20 % and under-reads an accelerating one by a few %.
pub const SPEED_TOL: f32 = 1.0;

/// Max plausible Health loss for one hit (head: the most permissive part).
/// `speed`: approach speed of the striking part (uu/s) from server history
/// (None = unknown → ceiling). `stab`: thrust / Inside contact.
pub fn loss_cap(class: WeaponClass, speed: Option<f32>, armour: Armour, stab: bool) -> f32 {
    loss_cap_bone(class, speed, armour, stab, "")
}

/// Armour layer `victim` wears over `bone` (None without a kit).
pub fn armour_on(victim: PeerId, bone: &str) -> Armour {
    victim_armour(&kits().lock().unwrap(), victim, bone)
}

// ---- Snap Neck ----------------------------------------------------------------

/// Last reported Neck Health per player (vitals frames): a strong head /
/// neck hit at Neck Health ≤ 25 snaps the neck (Health = 0), so such a claim
/// may book a lethal loss.
fn necks() -> &'static Mutex<HashMap<PeerId, f32>> {
    static N: OnceLock<Mutex<HashMap<PeerId, f32>>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Neck Health at or below which a lethal head / neck claim is plausible
/// (game threshold 25, +10 for the hit's own reduction and vitals lag).
pub const SNAP_NECK_HEALTH: f32 = 35.0;

pub fn note_neck_health(peer: PeerId, neck: f32) {
    if neck.is_finite() { necks().lock().unwrap().insert(peer, neck); }
}
fn neck_snappable(victim: PeerId) -> bool {
    necks().lock().unwrap().get(&victim).map_or(false, |n| *n <= SNAP_NECK_HEALTH)
}

// ---- per-player kit facts ----------------------------------------------------

#[derive(Debug, Clone, Default)]
struct KitFacts {
    weapons: Vec<WeaponClass>,
    /// slot group → layer
    armour: HashMap<String, Armour>,
}

fn kits() -> &'static Mutex<HashMap<PeerId, KitFacts>> {
    static K: OnceLock<Mutex<HashMap<PeerId, KitFacts>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember a player's resolved kit (glue: loadout.rs `KitStore::set_kit`,
/// which also assigns the default class kit to every peer that never sent
/// one). Class "none" (FREE mode, the game's own gear) is the one case where
/// the server cannot know the weapon: it is recorded as `Unknown` (the
/// longest / hardest-hitting class). A peer with NO record at all is judged
/// as a Sword ("no kit" must not be the most permissive class).
pub fn set_kit(peer: PeerId, kit: &KitSel) {
    let mut k = kits().lock().unwrap();
    if kit.class() == "none" {
        k.insert(peer, KitFacts { weapons: vec![WeaponClass::Unknown], armour: HashMap::new() });
        return;
    }
    let mut f = KitFacts::default();
    for w in [kit.r(), kit.l()] {
        f.weapons.push(class_of(w));
    }
    for a in kit.armor() {
        if let Some(item) = crate::loadout::catalog::item(a) {
            let l = layer_of(a);
            let e = f.armour.entry(item.group.to_string()).or_insert(Armour::None);
            if l > *e { *e = l; }
        }
    }
    k.insert(peer, f);
}

pub fn forget(peer: PeerId) {
    kits().lock().unwrap().remove(&peer);
    necks().lock().unwrap().remove(&peer);
}

/// Class assumed for a player the server has no kit record for: the
/// median class, never the most permissive one.
pub const NO_KIT_CLASS: WeaponClass = WeaponClass::Sword;

/// The more damaging weapon class `peer` holds (NO_KIT_CLASS without a kit
/// record, Unknown for the game's own gear).
pub fn weapon_class(peer: PeerId) -> WeaponClass {
    attacker_class(&kits().lock().unwrap(), peer)
}

/// The more damaging of the attacker's held weapons (see `weapon_class`).
fn attacker_class(k: &HashMap<PeerId, KitFacts>, attacker: PeerId) -> WeaponClass {
    let Some(f) = k.get(&attacker) else { return NO_KIT_CLASS };
    // An empty hand is not a weapon: body contacts are recognised by lag comp
    // (`Info::unarmed`) and capped as Unarmed separately.
    f.weapons.iter().copied().filter(|c| *c != WeaponClass::Unarmed)
        .max_by(|a, b| (rig_max(*a) * hit_vel_factor(*a)).total_cmp(&(rig_max(*b) * hit_vel_factor(*b))))
        .unwrap_or(WeaponClass::Unarmed)
}

fn victim_armour(k: &HashMap<PeerId, KitFacts>, victim: PeerId, bone: &str) -> Armour {
    let Some(f) = k.get(&victim) else { return Armour::None };
    groups_for_bone(bone).iter().filter_map(|g| f.armour.get(*g)).copied().max().unwrap_or(Armour::None)
}

// ---- applying the cap ----------------------------------------------------------

/// Health loss the attacker claims: −Health delta, or damage_out without deltas.
pub fn claimed_loss(hit: &DamageEvent) -> f32 {
    let d = hit.deltas().iter().find(|d| d.i == FIELD_HEALTH).map(|d| d.v);
    let loss = match d {
        Some(v) => -v,
        None if hit.deltas().is_empty() => hit.damage_out,
        None => 0.0,
    };
    if loss.is_finite() { loss.max(0.0) } else { 0.0 }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clamp {
    pub claimed: f32,
    pub cap: f32,
    /// Factor applied to the hit's damage fields (1.0 = untouched).
    pub factor: f32,
}

/// Cap for `hit` by `attacker` with the given server-measured contact speed.
pub fn cap_for(attacker: PeerId, hit: &DamageEvent, speed: Option<f32>) -> f32 {
    cap_for_as(attacker, hit, speed, false)
}

/// `cap_for`, with the striking part known to be the attacker's body
/// (`unarmed`: fist, elbow, knee — lag comp found no blade at the contact).
pub fn cap_for_as(attacker: PeerId, hit: &DamageEvent, speed: Option<f32>, unarmed: bool) -> f32 {
    let k = kits().lock().unwrap();
    let class = if unarmed { WeaponClass::Unarmed } else { attacker_class(&k, attacker) };
    let armour = victim_armour(&k, hit.target_peer_id, hit.bone_str());
    let stab = hit.flags & FLAG_STAB != 0 || hit.draw_cut > 0.0 || hit.flags & FLAG_INSIDE != 0;
    let speed = speed.or(Some(MAX_STRIKE_SPEED));
    let part = part_of(hit.bone_str());
    drop(k);
    if matches!(part, Part::Head | Part::Neck | Part::UpperTorso) && neck_snappable(hit.target_peer_id)
        && native_bound_part(class, speed.unwrap_or(MAX_STRIKE_SPEED) * SPEED_TOL, armour, stab, Part::Head) > 0.0
    {
        return LOSS_CEILING; // Snap Neck: Health = 0
    }
    loss_cap_bone(class, speed, armour, stab, hit.bone_str())
}

/// Striking speed assumed when lag comp measured none (legacy victim
/// without history): a fast two-handed swing's blade point, cm/s.
pub const MAX_STRIKE_SPEED: f32 = 4500.0;

/// Scale the hit's damage (Health and every other damage delta, raw and
/// output damage) so its Health loss is at most `cap`. The owner replays
/// raw_damage natively and tops up to the deltas (HSMPCombat EXACT_DELTAS),
/// so scaling all of them keeps the replay consistent.
pub fn apply_cap(hit: &mut DamageEvent, cap: f32) -> Clamp {
    let claimed = claimed_loss(hit);
    if claimed <= cap || claimed <= 0.0 {
        return Clamp { claimed, cap, factor: 1.0 };
    }
    let factor = (cap / claimed).clamp(0.0, 1.0);
    for d in hit.deltas_mut() {
        if d.v < 0.0 { d.v *= factor; }
    }
    // (Only the booked deltas: the impact inputs the owner replays are bounded
    // by the contact's physics in `clamp_impact` — solo parity.)
    Clamp { claimed, cap, factor }
}

/// Full damage-cap step for an accepted hit: compute the cap, clamp, count.
pub fn clamp_hit(attacker: PeerId, hit: &mut DamageEvent, speed: Option<f32>) -> Clamp {
    clamp_hit_as(attacker, hit, speed, false)
}

/// `clamp_hit` for a contact made by the attacker's body, not its weapon.
pub fn clamp_hit_as(attacker: PeerId, hit: &mut DamageEvent, speed: Option<f32>, unarmed: bool) -> Clamp {
    let cap = cap_for_as(attacker, hit, speed, unarmed);
    let c = apply_cap(hit, cap);
    if c.factor < 1.0 {
        use super::cheat::{self, Kind};
        cheat::bump(attacker, Kind::DamageClamped);
        cheat::add_clamped_hp(attacker, c.claimed - c.cap);
        if c.claimed > SEVERE * c.cap { cheat::bump(attacker, Kind::DamageSevere); }
        if super::rate::log_ok("damage_clamped") { // client-drivable, budgeted
            tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id, bone = hit.bone_str(),
                claimed = c.claimed, cap = c.cap, speed = ?speed, "combat: damage clamped");
        }
    }
    c
}

// ---- impact inputs (solo parity) ------------------------------------------------

/// Kick Power the game passes (Willie feet weapons use 10).
pub const KICK_MAX: f32 = 10.0;
/// Claim flags bit 5: the impact fields carry Deal Complex Damage inputs
/// (HSMPCombat FLAG_COMPLEX): impulse / velocity = Hit Impulse / Hit Velocity
/// (pre-armour), cutting_power / draw_cut pre-armour, pain_rate = Stab Rate,
/// damage_out = Rigidity, dism_blunt = Blunt Destruction Int | Kick·10 << 8 |
/// Lower Threshold << 16 | Extra High Velocity << 17. The owner replays them
/// through its OWN armour stage (solo parity).
pub const FLAG_COMPLEX: u8 = 1 << 5;
/// Hit Impulse ceiling over the attacker's peak striking speed (live user
/// swings: the DCD Hit Impulse — the normal impulse, which rubber
/// banding does not scale — reached ~11× it on a blade pressing into a body).
/// It only feeds Deal Complex Damage's contact gate and the knock-back.
pub const IMP_CEIL_K: f32 = 12.0;

fn len3(v: [f32; 3]) -> f32 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }

/// Smallest Willie height factor (Get Damage HM: 0.875 ..= 1.125).
pub const HM_MIN: f32 = 0.875;

/// The owner's native replay of an armour-stage claim (FLAG_COMPLEX), as the
/// decompiled Deal Complex Damage → Get Damage computes it: Raw through
/// `armour` from the approved Rigidity · Hit Velocity, cutting power and stab
/// rate; DRS = 2 · hm · Raw (gate DRS_GATE unless a stab); Health loss =
/// DRS · kH(part) · 0.00025. With `hm` = HM_MIN and the layer's weakest
/// piece it is what the ledger books (Snap Neck aside).
pub fn replay_loss(hit: &DamageEvent, armour: Armour, hm: f32) -> f32 {
    let (db, dc, ds) = protection(armour);
    let r = if hit.damage_out.is_finite() { hit.damage_out.max(0.0) } else { 0.0 };
    let v = len3(hit.velocity);
    let v = if v.is_finite() { v } else { 0.0 };
    let cp = if hit.cutting_power.is_finite() { hit.cutting_power.max(0.0) } else { 0.0 };
    let s = if hit.pain_rate.is_finite() { hit.pain_rate.clamp(0.0, 1.0) } else { 0.0 };
    let e = cp * v * r;
    let resist = (dc * (1.0 - s) + ds * s) * 1000.0;
    let frac = if e > 0.0 { ((e - resist) / e).max(0.0) } else { 0.0 };
    let raw = (r * v * (1.0 - frac) - 100.0 * db).max(0.0) + r * v * frac;
    let drs = 2.0 * hm * raw;
    if drs <= DRS_GATE && hit.flags & FLAG_STAB == 0 { return 0.0; }
    (drs * k_health(part_of(hit.bone_str())) * 0.00025).clamp(0.0, LOSS_CEILING)
}

/// Bound armour-stage inputs by the physics of this contact.
fn clamp_complex(class: WeaponClass, hit: &mut DamageEvent, striking: Option<f32>, relative: Option<f32>, rel_exact: bool, raw_max: f32) -> ImpactClamp {
    let cp = hit.cutting_power;
    let m = if cp >= 50.0 { 1.0 } else { 1.0 + 0.666 * (50.0 - cp) / 50.0 };
    let rig_cap = rig_max(class) * QUALITY_MAX * (1.0 + 2.0 * (m - 1.0));
    if !hit.damage_out.is_finite() { hit.damage_out = 0.0; }
    hit.damage_out = hit.damage_out.clamp(0.0, rig_cap);
    if !hit.pain_rate.is_finite() { hit.pain_rate = 0.0; }
    hit.pain_rate = hit.pain_rate.clamp(0.0, 1.0);
    let low = (hit.dism_blunt & 0xFF).clamp(0, 6);
    let kick = ((hit.dism_blunt >> 8) & 0xFF).clamp(0, (KICK_MAX * 10.0) as i32);
    let lower = (hit.dism_blunt >> 16) & 1;
    // Extra High Velocity is for fired ammunition only (no projectiles here).
    hit.dism_blunt = low | (kick << 8) | (lower << 16);
    // The inputs are the attacker's OWN native Deal Complex Damage call — the
    // game's solo computation for that contact — and pass unchanged unless
    // physically impossible. A two-way rescale toward lag comp's relative
    // speed (×1/3..×3, plus "Hit Velocity follows the impulse") moves real
    // blows by up to 12× either way (measured in game): a rubber-banded frame
    // has Hit Velocity = 0.1 · impulse (live: vel 381, imp 3169), and the
    // instantaneous blade speed at the touch reads a blade already stopping in
    // its target (11..35 uu/s on clean hits → capped to nothing). `striking`
    // is lag comp's PEAK striking speed over the frames before the contact.
    let s = striking.unwrap_or(MAX_STRIKE_SPEED).min(MAX_STRIKE_SPEED);
    let rel = relative.unwrap_or(s).min(MAX_STRIKE_SPEED);
    let mut factor = 1.0f32;
    let base = s.max(rel);
    // 1) Servo over-read only (down only, exact geometry only): a stand-in
    //    relative speed (raw_damage) above anything the attacker's blade did
    //    (peak) and the real relative motion is the servo body rushing into
    //    the blade; the normal impulse scales with it (and Hit Velocity when it
    //    set it, the weapon's own COM speed ≈ 0.85 of its point's staying).
    let vrs = hit.raw_damage;
    let ilen = len3(hit.impulse);
    let vlen = len3(hit.velocity);
    if rel_exact && vrs.is_finite() && vrs > base && base > 0.0 && ilen.is_finite() && ilen > 0.0 {
        let k = base / vrs;
        if vlen > 0.0 && vlen <= ilen * 1.01 {
            let kv = (ilen * k).max(vlen.min(0.85 * base)) / vlen;
            hit.velocity = hit.velocity.map(|x| x * kv.min(1.0));
        }
        hit.impulse = hit.impulse.map(|x| x * k);
        factor = k;
    }
    // 2) Hard ceilings: Hit Velocity = max(weapon COM ≤ the point's peak,
    //    normal impulse ≤ hvf · relative speed); Hit Impulse ≤ IMP_CEIL_K · base.
    let (vmax, imax) = (s.max(hit_vel_factor(class) * rel), IMP_CEIL_K * base);
    for (v, cap) in [(&mut hit.h.velocity, vmax), (&mut hit.h.impulse, imax)] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l.is_finite() && l > cap && l > 0.0 {
            let k = cap / l;
            factor = factor.min(k);
            for x in v.iter_mut() { *x *= k; }
        } else if !l.is_finite() {
            *v = [0.0; 3];
        }
    }
    // raw_damage now holds Raw for the Get Damage fallback (unarmoured).
    hit.raw_damage = (hit.damage_out * len3(hit.velocity)).min(raw_max);
    ImpactClamp { raw_max, factor }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactClamp {
    pub raw_max: f32,
    /// Factor applied to raw / impulse / velocity (1.0 = untouched).
    pub factor: f32,
}

/// Upper bound of the Raw the game computes for this contact (Deal Complex
/// Damage, unarmoured): R_eff · HitVel with
///   R_eff  ≤ rig_max(class) · 1.1 · (1 + 2·(M − 1)),  M = MapRange(CP, 50..0 → 1..1.666)
///          (the claim's cutting power: sharp aligned edges get NO blunt bonus)
///   HitVel ≤ max(attacker striking speed, hit_vel_factor · relative speed)
/// where the relative speed is the attacker's REAL striking point against the
/// victim's REPLICATED body (lag comp), not the attacker's servo-driven
/// stand-in, whose correction velocities inflate the measured impact.
pub fn raw_max(class: WeaponClass, cutting_power: f32, striking: Option<f32>, relative: Option<f32>) -> f32 {
    let cp = if cutting_power.is_finite() { cutting_power.clamp(0.0, 200.0) } else { 0.0 };
    let m = if cp >= 50.0 { 1.0 } else { 1.0 + 0.666 * (50.0 - cp) / 50.0 };
    let r = rig_max(class) * QUALITY_MAX * (1.0 + 2.0 * (m - 1.0));
    let s = striking.unwrap_or(MAX_STRIKE_SPEED).min(MAX_STRIKE_SPEED);
    let rel = relative.unwrap_or(s).min(MAX_STRIKE_SPEED);
    r * s.max(hit_vel_factor(class) * rel)
}

/// Clamp the impact inputs the owner replays natively (raw damage, impulse,
/// velocity, cutting power, draw cut, dismemberment level) to what the game
/// could compute for this contact. The owner's own Get Damage then produces
/// solo-equivalent damage from them.
pub fn clamp_impact(attacker: PeerId, hit: &mut DamageEvent, striking: Option<f32>, relative: Option<f32>, unarmed: bool) -> ImpactClamp {
    clamp_impact_ex(attacker, hit, striking, relative, true, unarmed)
}

/// `clamp_impact` with lag comp's `Info::rel_exact` (false: the relative speed
/// is the v1 blade estimate — no rescale toward it, hard caps only).
pub fn clamp_impact_ex(attacker: PeerId, hit: &mut DamageEvent, striking: Option<f32>, relative: Option<f32>, rel_exact: bool, unarmed: bool) -> ImpactClamp {
    let class = if unarmed { WeaponClass::Unarmed } else { attacker_class(&kits().lock().unwrap(), attacker) };
    hit.cutting_power = if hit.cutting_power.is_finite() { hit.cutting_power.clamp(0.0, 200.0) } else { 0.0 };
    hit.draw_cut = if hit.draw_cut.is_finite() { hit.draw_cut.clamp(0.0, 400.0) } else { 0.0 };
    let rm = raw_max(class, hit.cutting_power, striking, relative);
    if hit.flags & FLAG_COMPLEX != 0 {
        return clamp_complex(class, hit, striking, relative, rel_exact, rm);
    }
    hit.dism_blunt = hit.dism_blunt.clamp(0, 6);
    let mut factor = 1.0f32;
    if hit.raw_damage > rm && hit.raw_damage > 0.0 {
        factor = (rm / hit.raw_damage).clamp(0.0, 1.0);
        hit.raw_damage = rm;
    }
    // |Velocity| = HitVel·R·Kick and |Impulse| = HitImpulse·R·Kick: same bound.
    let vmax = rm * KICK_MAX;
    for v in [&mut hit.h.velocity, &mut hit.h.impulse] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let k = if l > 0.0 { (factor).min(vmax / l) } else { 1.0 };
        for x in v.iter_mut() { *x *= k; }
    }
    if factor < 0.99 {
        tracing::debug!(attacker, hit_id = hit.hit_id, raw_max = rm, factor, ?striking, ?relative,
            "combat: impact inputs clamped (stand-in measurement above the physical bound)");
    }
    ImpactClamp { raw_max: rm, factor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_from_catalogue_ids() {
        assert_eq!(class_of("w_longsword1"), WeaponClass::Sword);
        assert_eq!(class_of("w_messer_a"), WeaponClass::Sword);
        assert_eq!(class_of("w_rondel"), WeaponClass::Dagger);
        assert_eq!(class_of("w_poleaxe_m"), WeaponClass::Polearm);
        assert_eq!(class_of("w_waraxe3"), WeaponClass::Axe);
        assert_eq!(class_of("w_hammer_ll"), WeaponClass::Blunt);
        assert_eq!(class_of("t_pitchfork_a"), WeaponClass::Polearm);
        assert_eq!(class_of(""), WeaponClass::Unarmed);
        // Every catalogue weapon maps to a known class.
        for it in crate::loadout::catalog::ITEMS {
            if it.kind == crate::loadout::catalog::Kind::Weapon {
                assert_ne!(class_of(it.id), WeaponClass::Unknown, "{}", it.id);
            }
        }
        assert_eq!(layer_of("c_cuirass1"), Armour::Plate);
        assert_eq!(layer_of("m_hauberk"), Armour::Mail);
        assert_eq!(layer_of("b_gambeson"), Armour::Padded);
        assert_eq!(layer_of("h_hat2"), Armour::None);
    }

    /// A peer the server holds no kit for is a Sword (median),
    /// never the most permissive class; only the explicit game-gear choice
    /// ("none", FREE mode) is Unknown.
    #[test]
    fn no_kit_is_the_median_class_not_the_maximum() {
        let p = 0x4E11;
        forget(p);
        assert_eq!(weapon_class(p), WeaponClass::Sword);
        assert_eq!(crate::validate::pose::geom(weapon_class(p)).blade_max, 125.0);
        let none = KitSel::new("none", "", "", &[], [0; 4]);
        set_kit(p, &none);
        assert_eq!(weapon_class(p), WeaponClass::Unknown, "game's own gear: unknown weapon");
        let poleaxe = KitSel::new("custom", "w_poleaxe_m", "", &[], [0; 4]);
        set_kit(p, &poleaxe);
        assert_eq!(weapon_class(p), WeaponClass::Polearm);
        forget(p);
        assert_eq!(weapon_class(p), WeaponClass::Sword);
    }

    #[test]
    fn cap_monotonic_in_speed_and_armour() {
        let cap = |c, v: f32, a, bone| loss_cap_bone(c, Some(v), a, false, bone);
        // Below the DRS gate a contact does no Health damage at all.
        assert_eq!(cap(WeaponClass::Sword, 50.0, Armour::None, "spine_03"), LOSS_FLOOR_MEASURE);
        // A fast sword cut on the chest: Raw ≤ 0.85·1.1·2.333·1.8·v.
        let chest = cap(WeaponClass::Sword, 2000.0, Armour::None, "spine_03");
        let head = cap(WeaponClass::Sword, 2000.0, Armour::None, "head");
        let arm = cap(WeaponClass::Sword, 2000.0, Armour::None, "lowerarm_l");
        assert!(chest > 20.0 && chest < 60.0, "{}", chest);
        assert!(head > 90.0, "a fast sword cut to the head can nearly kill: {}", head);
        assert!(arm < 3.0, "limbs lose part health, not Health: {}", arm);
        // Monotonic in speed, armour lowers it, heavier classes raise it.
        assert!(cap(WeaponClass::Sword, 1000.0, Armour::None, "spine_03") < chest);
        let plate = cap(WeaponClass::Sword, 1000.0, Armour::Plate, "spine_03");
        assert!(plate < cap(WeaponClass::Sword, 1000.0, Armour::None, "spine_03"), "{}", plate);
        assert!(cap(WeaponClass::Blunt, 1000.0, Armour::Plate, "spine_03") > plate);
        // Unknown speed and kit: the permissive ceiling.
        assert_eq!(loss_cap(WeaponClass::Unknown, None, Armour::None, false), LOSS_CEILING);
        // Stabs skip the DRS gate.
        assert!(native_bound_part(WeaponClass::Dagger, 140.0, Armour::None, true, Part::UpperTorso)
            > native_bound_part(WeaponClass::Dagger, 140.0, Armour::None, false, Part::UpperTorso));
        // Parts follow the game's kH table.
        assert_eq!(part_of("neck_01"), Part::Neck);
        assert_eq!(part_of("spine_02"), Part::LowerTorso);
        assert_eq!(part_of("spine_04"), Part::UpperTorso);
        assert_eq!(part_of("hand_r"), Part::ArmR);
        assert_eq!(part_of("calf_l"), Part::LegL);
    }

    #[test]
    fn armour_formula_matches_deal_complex_damage() {
        // Decompile example: sword CP 100, R 0.8, V 1500 on plate (B15 C250):
        // E = 120000 < resist 250000 → frac 0 → max(0, 1200 − 1500) = 0. Our
        // bound takes CP = 200 and the weakest plate (B10 C200): E = 240000
        // > 200000 → frac 1/6 → max(0, 1000 − 1000) + 200 = 200 for a cut;
        // The stab branch (s = 1, DefStab 75 → resist 75000) is worse: frac
        // 165000/240000 → 0 + 1200·0.6875 = 825.
        let r = raw_through(0.8, 1500.0, Armour::Plate);
        assert!((r - 825.0).abs() < 1.0, "{}", r);
        // Gambeson (B5 C20): frac = (240000 − 20000)/240000.
        let g = raw_through(0.8, 1500.0, Armour::Padded);
        assert!(g > 1000.0 && g <= 1200.0, "{}", g);
        assert_eq!(raw_through(0.8, 1500.0, Armour::None), 1200.0);
    }

    #[test]
    fn snap_neck_allows_a_lethal_head_claim() {
        let p = 9_701;
        set_kit(p, &KitSel::new("custom", "w_arming1", "", &[], [0; 4]));
        let mut h = DamageEvent::new(crate::proto::Damage {
            hit_id: 1, target_peer_id: 9_702, round: 1, bone: hsmp_ipc::layout::Str::new("neck_01"), offset: [0.0; 3], location: [0.0; 3],
            impulse: [0.0; 3], velocity: [0.0; 3], normal: [0.0; 3], raw_damage: 1.0, cutting_power: 1.0, pain_rate: 1.0,
            draw_cut: 0.0, damage_out: 0.0, dism_blunt: 0, flags: 0, age_ms: 0,
            attacker_ts: 0, victim_view_ts: 0, victim_arm_ts: 0, ..Default::default()
        }, &crate::proto::deltas_of(&[(FIELD_HEALTH, -100.0)]));
        assert!(cap_for(p, &h, Some(1500.0)) < 60.0);
        note_neck_health(9_702, 20.0);
        assert_eq!(cap_for(p, &h, Some(1500.0)), LOSS_CEILING);
        h.set_bone("calf_l");
        assert!(cap_for(p, &h, Some(1500.0)) < 10.0);
    }
}
