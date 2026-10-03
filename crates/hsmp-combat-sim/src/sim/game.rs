//! What the game's own damage math does for one contact — the "native"
//! Health loss the attacker's HSMPCombat measures on its stand-in and
//! claims. An INDEPENDENT implementation of the decompiled formula
//! (Willie_BP "Get Damage" / "Deal Complex Damage", ModularWeaponBP
//! "Collision Hit"; docs/development/subsystems/combat.md), so the server's table
//! (`validate::damage`) is checked against it rather than against itself:
//!
//!   Raw = R_eff·V through the armour layer, DRS = 2·hm·Raw,
//!   Health −= DRS·kH[part]·0.00025 when DRS > 1000 (or a stab / Inside).
//!
//! Per weapon (rig tag, quality, mass ≈ impulse/velocity) the sim draws the
//! unknowns the server must bound: blade alignment, Willie height.

use super::Rng;
use crate::validate::damage::{Armour, WeaponClass};

/// Upper end of each class's rig tags (weapon_tags / module_tags).
pub fn rig(c: WeaponClass) -> f32 {
    match c {
        WeaponClass::Sword => 0.85,
        WeaponClass::Dagger => 1.00,
        WeaponClass::Axe => 1.25,
        WeaponClass::Blunt => 2.00,
        WeaponClass::Polearm => 1.45,
        WeaponClass::Shield => 1.00,
        WeaponClass::Unarmed => 1.00,
        WeaponClass::Unknown => 2.50,
    }
}

/// max(|COM velocity|, |normal impulse|) / striking-point speed.
pub fn hit_vel(c: WeaponClass) -> f32 {
    match c {
        WeaponClass::Dagger => 1.2,
        WeaponClass::Sword => 1.8,
        WeaponClass::Unarmed | WeaponClass::Unknown => 3.0,
        _ => 2.5,
    }
}

/// Protection (Blunt, Cut, Stab) of the weakest piece of a layer.
fn prot(a: Armour) -> (f32, f32, f32) {
    match a {
        Armour::None => (0.0, 0.0, 0.0),
        Armour::Padded => (5.0, 20.0, 2.0),
        Armour::Mail => (1.0, 150.0, 15.0),
        Armour::Plate => (10.0, 200.0, 75.0),
    }
}

pub fn k_health(bone: &str) -> f32 {
    let b = bone;
    if b.starts_with("head") { 15.0 }
    else if b.contains("neck") { 7.5 }
    else if b.starts_with("pelvis") || b.starts_with("spine_01") || b.starts_with("spine_02") { 2.5 }
    else if b.contains("arm") || b.starts_with("hand") || b.starts_with("thigh") || b.starts_with("calf") || b.starts_with("foot") { 0.1 }
    else { 5.0 }
}

/// Deal Complex Damage for cutting power `cp` (0..200) and stab rate `s`.
pub fn deal(r: f32, v: f32, a: Armour, cp: f32, s: f32) -> f32 {
    let (db, dc, ds) = prot(a);
    let e = cp * v * r;
    let resist = (dc * (1.0 - s) + ds * s) * 1000.0;
    let frac = if e > 0.0 { ((e - resist) / e).max(0.0) } else { 0.0 };
    (r * v * (1.0 - frac) - 100.0 * db).max(0.0) + r * v * frac
}

/// The physically possible maximum Health loss of one Get Damage call
/// (best quality, fully aligned blunt contact, tallest Willie, any cutting
/// power): the reference for "inflated" claims.
pub fn bound(class: WeaponClass, speed: f32, armour: Armour, stab: bool, bone: &str) -> f32 {
    let r = rig(class) * 1.1 * 2.333;
    let v = speed * hit_vel(class);
    let raw = [0.0f32, 1.0].iter().map(|&s| deal(r, v, armour, 200.0, s)).fold(0.0, f32::max);
    let drs = 2.0 * 1.125 * raw;
    if drs <= 1000.0 && !stab { return 0.0; }
    (drs * k_health(bone) * 0.00025).min(100.0)
}

/// One honest contact: the formula with the unknowns drawn (alignment,
/// quality, cutting power, Willie height, impulse vs velocity).
pub fn native_loss(class: WeaponClass, speed: f32, armour: Armour, stab: bool, bone: &str, rng: &mut Rng) -> f32 {
    let align = rng.rf(0.0, 2.0);                       // edge + tip alignment
    let cp = rng.rf(0.0, 200.0);                        // cutting rate
    let m = if cp >= 50.0 { 1.0 } else { 1.0 + 0.666 * (50.0 - cp) / 50.0 };
    let r = rig(class) * rng.rf(0.9, 1.1) * (1.0 + align * (m - 1.0)).max(0.1);
    let v = speed * rng.rf(0.8, 1.0) * hit_vel(class);
    let s = if stab { 1.0 } else { rng.rf(0.0, 0.3) };
    let raw = deal(r, v, armour, cp, s);
    let hm = rng.rf(0.875, 1.125);
    let drs = 2.0 * hm * raw;
    if drs <= 1000.0 && !stab { return 0.0; }
    (drs * k_health(bone) * 0.00025).min(100.0)
}

/// HSMPCombat picks a merged claim's bone by this rank (the game's kH order:
/// the part whose Health loss dominates), then by raw damage.
pub fn part_rank(bone: &str) -> u8 {
    let k = k_health(bone);
    if k >= 15.0 { 5 } else if k >= 7.5 { 4 } else if k >= 5.0 { 3 } else if k >= 2.5 { 2 } else { 1 }
}

/// Armour layer a fighter wears over `bone` (kit → catalogue groups → layer;
/// the strongest layer covering the bone).
pub fn armour_over(armour: &[&str], bone: &str) -> Armour {
    use crate::validate::damage::{groups_for_bone, layer_of};
    let groups = groups_for_bone(bone);
    armour.iter()
        .filter(|id| crate::loadout::catalog::item(id).map_or(false, |i| groups.contains(&i.group)))
        .map(|id| layer_of(id))
        .max()
        .unwrap_or(Armour::None)
}

/// Per-field deltas the attacker's stand-in measurement reports for one
/// Get Damage call (HSMPCombat FIELDS indices), from the decompiled formula:
/// Health −= DRS·kH·0.00025 (gated), part health −= DRS·kL·0.0025,
/// Consciousness −= F·kC·0.005 (F ≈ DRS), Pain += DRS·0.001·(1..10),
/// Damage Taken += 0.1·hp, and the bookkeeping fields Get Damage writes:
/// Sustained Damage and Last Damage Taken (= DRS, thousands).
pub struct Fields {
    pub drs: f32,
    pub d: Vec<(u8, f32)>,
}

fn part_field(bone: &str) -> (u8, f32, f32) {
    // (FIELDS index of the part health, kL, kC)
    let r = bone.ends_with("_r");
    if bone.starts_with("head") { (1, 3.0, 10.0) }
    else if bone.contains("neck") { (2, 1.0, 1.0) }
    else if bone.starts_with("pelvis") || bone.starts_with("spine_01") || bone.starts_with("spine_02") { (4, 0.5, 0.5) }
    else if bone.contains("arm") || bone.starts_with("hand") { (if r { 6 } else { 7 }, 1.0, 0.0) }
    else if bone.starts_with("thigh") || bone.starts_with("calf") || bone.starts_with("foot") { (if r { 8 } else { 9 }, 1.0, 0.0) }
    else { (3, 1.0, 1.0) }
}

/// DRS of a Health loss on `bone` (inverse of the Health formula; a gated or
/// sub-1000 contact still writes its DRS to the bookkeeping fields).
pub fn fields_for(loss: f32, speed: f32, class: WeaponClass, bone: &str, rng: &mut Rng) -> Fields {
    let kh = k_health(bone);
    let drs = if loss > 0.0 { loss / (kh * 0.00025) } else { 2.0 * rig(class) * speed * hit_vel(class) * rng.rf(0.3, 0.9) };
    let (pi, kl, kc) = part_field(bone);
    let mut d = Vec::new();
    if loss > 0.0 { d.push((0u8, -loss)); }
    if drs > 1000.0 { d.push((pi, -(drs * kl * 0.0025).min(100.0))); }
    if kc > 0.0 { d.push((10u8, -(drs * kc * 0.005).min(100.0))); }
    d.push((13u8, drs * 0.001 * rng.rf(1.0, 10.0)));
    if loss > 0.0 { d.push((16u8, (0.1 * loss).min(1.0))); }
    d.push((15u8, drs));
    d.push((17u8, drs));
    Fields { drs, d }
}

/// Health loss from a DRS on `bone` (game gate: DRS > 1000 unless a stab).
pub fn health_loss(drs: f32, stab: bool, bone: &str) -> f32 {
    if drs <= 1000.0 && !stab { return 0.0; }
    (drs * k_health(bone) * 0.00025).min(100.0)
}

/// The hidden per-contact physics both solo and MP share (one draw).
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Rigidity after alignment (R_eff), cutting power, stab rate, Willie height factor.
    pub r: f32,
    pub cp: f32,
    pub s: f32,
    pub hm: f32,
    /// Impulse per unit relative speed (μ(1+e)) and weapon COM speed.
    pub kappa: f32,
    pub vcom: f32,
}

pub fn draw_contact(class: WeaponClass, point_speed: f32, stab: bool, rng: &mut Rng) -> Contact {
    let align = rng.rf(0.0, 2.0);
    let cp = rng.rf(0.0, 200.0);
    let m = if cp >= 50.0 { 1.0 } else { 1.0 + 0.666 * (50.0 - cp) / 50.0 };
    Contact {
        r: rig(class) * rng.rf(0.9, 1.1) * (1.0 + align * (m - 1.0)).max(0.1),
        cp,
        s: if stab { 1.0 } else { rng.rf(0.0, 0.3) },
        hm: rng.rf(0.875, 1.125),
        kappa: hit_vel(class) * rng.rf(0.6, 1.0),
        vcom: point_speed * rng.rf(0.75, 0.95),
    }
}

/// Health loss of `k` at relative speed `vrel` through `armour` on `bone`.
/// Returns (loss, HitVel, normal impulse).
pub fn contact_loss(k: &Contact, vrel: f32, armour: Armour, stab: bool, bone: &str) -> (f32, f32, f32) {
    let ni = k.kappa * vrel;
    let hv = k.vcom.max(ni);
    let raw = deal(k.r, hv, armour, k.cp, k.s);
    (health_loss(2.0 * k.hm * raw, stab, bone), hv, ni)
}
