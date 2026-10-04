//! hit_vel_factor calibration: log parsing and the recommendation (src/calib.rs).

use hsmp_combat_sim::calib::*;
use hsmp_combat_sim::validate::damage::WeaponClass;

#[test]
fn parses_the_game_and_server_lines() {
    let g = "[Lua] [HSMPParity] 12:00:01 DCD on Willie_BP_C_3 bone=spine_03 by ModularWeaponBP_C wp=ModularWeaponBP_Mace_T2_C_7: |vel|=2650 |imp|=2650 rel=1100 cut=0.0 stab=0.00 rig=3.10 kick=1.0";
    let s = parse_parity_line(g).unwrap();
    assert_eq!((s.class, s.vel, s.imp, s.rel, s.source), (WeaponClass::Blunt, 2650.0, 2650.0, 1100.0, Source::Game));
    let fist = "[HSMPParity] DCD on Willie_BP_C_3 bone=head by Willie_BP_C: |vel|=900 |imp|=1400 rel=700 cut=8.0";
    assert_eq!(parse_parity_line(fist).unwrap().class, WeaponClass::Unarmed);
    let srv = "2026-10-04T12:00:00Z  INFO hsmp_server::combat: combat: impact rescale attacker=3 target=4 hit_id=9 bone=\"spine_03\" class=Sword vel_claimed=2900.5 vel_forwarded=2700 imp_claimed=3100 imp_forwarded=3100 standin_rel=1500 server_striking=Some(1450.0) server_peak=Some(1500.0) server_relative=Some(1400.0) rel_exact=true factor=0.93";
    let s = parse_rescale_line(srv).unwrap();
    assert_eq!((s.class, s.vel, s.rel, s.peak), (WeaponClass::Sword, 2900.5, 1400.0, Some(1500.0)));
    assert!(parse_rescale_line(&srv.replace("server_relative=Some(1400.0)", "server_relative=None")).is_none());
    assert!(parse_rescale_line(&srv.replace("class=Sword ", "")).is_none(), "lines without a class are not guessed");
    assert_eq!(parse_log(&format!("{g}\nnoise\n{srv}\n")).len(), 2);
}

#[test]
fn needed_factor_and_recommendation() {
    let mk = |vel: f32, rel: f32, peak: Option<f32>| Sample {
        source: Source::Server, class: WeaponClass::Sword, vel, imp: vel, rel, peak, true_rel: None,
    };
    assert_eq!(mk(3000.0, 1000.0, Some(1500.0)).needed(), Some(3.0));
    assert_eq!(mk(1400.0, 1000.0, Some(1500.0)).needed(), None, "the peak striking speed covers it");
    assert_eq!(mk(900.0, 100.0, None).needed(), None, "relative speed too small to judge");
    assert!(mk(3000.0, 1000.0, Some(1500.0)).clamped_at(1.8) && !mk(1700.0, 1000.0, Some(1500.0)).clamped_at(1.8));
    // an under-read relative speed: the shipped ceiling cuts 2600, hvf · max(peak, rel) would not
    assert!(mk(2600.0, 1000.0, Some(1500.0)).clamped_at(1.8) && !mk(2600.0, 1000.0, Some(1500.0)).clamped_alt(1.8));
    // 1000 blows needing 1.0..2.0 evenly: p99.9 ≈ 2.0, recommendation 2.2.
    let s: Vec<Sample> = (0..1000).map(|i| mk(2000.0 + 2.0 * i as f32, 2000.0, Some(1500.0))).collect();
    let r = &report(&s)[0];
    assert_eq!((r.n, r.n_binding), (1000, 1000));
    assert!((r.p999 - 1.998).abs() < 1e-3 && (r.recommended.unwrap() - 2.198).abs() < 1e-2, "{r:?}");
    assert!((r.clamped_shipped - 0.2).abs() < 0.01, "needed > 1.8 for the top 20 %: {}", r.clamped_shipped);
    assert_eq!(report(&s[..50])[0].recommended, None, "too few samples");
    assert!(table(std::slice::from_ref(r)).contains("| Server | Sword | 1000 |"));
}

/// The calibration's ceiling is the server's: for blows over a grid of speeds the real
/// `clamp_impact_ex` cuts Hit Velocity exactly when `Sample::clamped_at` says so.
#[test]
fn the_calibrated_ceiling_is_the_servers() {
    use hsmp_combat_sim::loadout::KitSel;
    use hsmp_combat_sim::validate::damage::{clamp_impact_ex, hit_vel_factor, set_kit, FLAG_COMPLEX};
    let p = 9_901;
    set_kit(p, &KitSel::new("custom", "w_arming1", "", &[], [0; 4]));
    let hvf = hit_vel_factor(WeaponClass::Sword);
    let mut n = 0;
    for vel in [300.0f32, 900.0, 1600.0, 2400.0, 3100.0, 5000.0] {
        for rel in [250.0f32, 700.0, 1500.0, 2600.0] {
            for peak in [400.0f32, 1200.0, 2000.0] {
                let mut h = hsmp_combat_sim::proto::DamageEvent::new(hsmp_combat_sim::proto::Damage {
                    target_peer_id: 9_902, velocity: [vel, 0.0, 0.0], impulse: [vel, 0.0, 0.0], damage_out: 0.85,
                    cutting_power: 60.0, raw_damage: 0.0, flags: FLAG_COMPLEX, ..Default::default()
                }, &[]);
                h.flags |= FLAG_COMPLEX;
                clamp_impact_ex(p, &mut h, Some(peak), Some(rel), true, false);
                let s = Sample { source: Source::Server, class: WeaponClass::Sword, vel, imp: vel, rel, peak: Some(peak), true_rel: None };
                assert_eq!(h.velocity[0] < vel, s.clamped_at(hvf), "vel {vel} rel {rel} peak {peak}");
                n += 1;
            }
        }
    }
    assert_eq!(n, 72);
}
