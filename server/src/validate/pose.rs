//! Pose plausibility: limits on what
//! a client may stream about its OWN body and weapon, used by lag comp to
//! refuse reach-extension cheats (a hand streamed 2 m in front of the chest,
//! a blade streamed longer than the weapon).
//!
//! Weapon geometry is per damage class (`validate::damage::WeaponClass`)
//! because the catalogue has no lengths yet. `blade_max` bounds the streamed
//! blade segment (POSE codec v2: blade base → tip in world space) and
//! `grip_max` the distance from the nearer hand to that segment. Values are
//! the longest catalogue member of each class plus 15 % (stand-in retarget,
//! codec quantisation) — a false rejection here would be an honest player's
//! hit lost, a missed one only lets a cheater reach as far as the longest
//! weapon of the class.

use super::damage::WeaponClass;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponGeom {
    /// Longest plausible streamed blade segment, uu.
    pub blade_max: f32,
    /// Longest plausible hand → blade-segment distance, uu.
    pub grip_max: f32,
    /// Typical striking-segment length when the stream does not carry one
    /// (degenerate POSE blade: polearm tip scene at the grip).
    pub nominal: f32,
}

pub fn geom(class: WeaponClass) -> WeaponGeom {
    let g = |blade_max: f32, grip_max| WeaponGeom { blade_max, grip_max, nominal: blade_max / 1.15 };
    match class {
        // Kit unknown: the longest weapon in the game (poleaxe / spear).
        WeaponClass::Unknown => g(260.0, 220.0),
        // Fist/forearm "blade" (no weapon streamed normally).
        WeaponClass::Unarmed => g(45.0, 30.0),
        // Rondel / dagger blades 25–35 uu.
        WeaponClass::Dagger => g(50.0, 40.0),
        // Arming sword … bastard sword / messer: blades 70–105 uu.
        WeaponClass::Sword => g(125.0, 45.0),
        // Hatchet … war axe: haft 60–90, head at the end.
        WeaponClass::Axe => g(120.0, 90.0),
        // Maces, hammers, flails, tools.
        WeaponClass::Blunt => g(130.0, 90.0),
        // Spears, halberds, poleaxes, pitchforks: hafts up to ~210 uu, the
        // hands anywhere along them.
        WeaponClass::Polearm => g(260.0, 220.0),
        // Bucklers … pavises.
        WeaponClass::Shield => g(110.0, 60.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_classes_allow_more() {
        assert!(geom(WeaponClass::Dagger).blade_max < geom(WeaponClass::Sword).blade_max);
        assert!(geom(WeaponClass::Sword).blade_max < geom(WeaponClass::Polearm).blade_max);
        assert_eq!(geom(WeaponClass::Unknown), geom(WeaponClass::Polearm));
    }
}
