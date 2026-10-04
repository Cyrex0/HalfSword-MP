//! Servo parity: the native servo (`hsmp_native::servo::servo`) against Lua's `PURE.servo`
//! taken verbatim from mods/HSMPAvatars/Scripts/avatars_pure.lua (golden vectors generated here, both
//! sides fed the same numbers, results compared bit for bit).

use std::ffi::{CStr, CString};

use hsmp_native::servo::servo;
use mlua::ffi;

/// The Lua source of PURE.qmul .. PURE.servo (qmul, qrot, qconj, qangle, servo).
fn pure_servo_src() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/HSMPAvatars/Scripts/avatars_pure.lua");
    let s = std::fs::read_to_string(p).expect("HSMPAvatars avatars_pure.lua");
    let a = s.find("function PURE.qmul(").expect("PURE.qmul in HSMPAvatars");
    let b = s[a..].find("function PURE.fk_retarget(").expect("PURE.fk_retarget after PURE.servo") + a;
    s[a..b].to_string()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
    fn quat(&mut self) -> [f64; 4] {
        let q = [self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0)];
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    }
}

fn lua_num_list(v: &[f64]) -> String {
    // %.17g round-trips every f64 exactly.
    v.iter().map(|x| format!("{:.17e}", x)).collect::<Vec<_>>().join(",")
}

#[test]
fn native_servo_matches_pure_servo_bit_for_bit() {
    unsafe {
        let l = ffi::luaL_newstate();
        ffi::luaL_openlibs(l);
        let code = CString::new(format!("PURE = {{}}\n{}", pure_servo_src())).unwrap();
        assert_eq!(ffi::luaL_loadstring(l, code.as_ptr()), 0);
        assert_eq!(ffi::lua_pcall(l, 0, 0, 0), 0, "PURE servo source loads");
        let mut rng = Rng(0x5eed_1234);
        let mut checked = 0;
        for case in 0..4000 {
            let cq = rng.quat();
            let mut tq = rng.quat();
            if case % 7 == 0 {
                tq = cq; // no rotation error (s below 1e-9)
            }
            if case % 5 == 0 {
                tq = [-tq[0], -tq[1], -tq[2], -tq[3]]; // the e[4] < 0 branch
            }
            let cur = [rng.range(-5000.0, 5000.0), rng.range(-5000.0, 5000.0), rng.range(-200.0, 400.0), cq[0], cq[1], cq[2], cq[3]];
            let near = case % 3 == 0;
            let off = if near { 0.5 } else { 80.0 };
            let tg = [
                cur[0] + rng.range(-off, off),
                cur[1] + rng.range(-off, off),
                cur[2] + rng.range(-off, off),
                tq[0],
                tq[1],
                tq[2],
                tq[3],
                rng.range(-800.0, 800.0),
                rng.range(-800.0, 800.0),
                rng.range(-800.0, 800.0),
                rng.range(-900.0, 900.0),
                rng.range(-900.0, 900.0),
                rng.range(-900.0, 900.0),
            ];
            let com = [rng.range(-20.0, 20.0), rng.range(-20.0, 20.0), rng.range(-20.0, 20.0)];
            let dt = [1.0 / 60.0, 1.0 / 30.0, 0.0071, 0.08][case % 4];
            let (cap_lin, cap_ang) = if case % 2 == 0 { (900.0, 900.0) } else { (rng.range(5.0, 200.0), rng.range(5.0, 200.0)) };
            let gain = [1.0, 0.8, 0.35, 0.15][case % 4];
            let (v, dl, gl) = servo(&cur, &tg, com, dt, cap_lin, cap_ang, gain);
            let expr = format!(
                "local r = {{ PURE.servo({{{}}}, {{{}}}, {{{}}}, {}, {}, {}, {}) }}\nreturn string.format(\"%a %a %a %a %a %a %a %a\", table.unpack(r))",
                lua_num_list(&cur),
                lua_num_list(&tg),
                lua_num_list(&com),
                lua_num_list(&[dt]),
                lua_num_list(&[cap_lin]),
                lua_num_list(&[cap_ang]),
                lua_num_list(&[gain])
            );
            let c = CString::new(expr).unwrap();
            assert_eq!(ffi::luaL_loadstring(l, c.as_ptr()), 0);
            assert_eq!(ffi::lua_pcall(l, 0, 1, 0), 0);
            let got = CStr::from_ptr(ffi::lua_tostring(l, -1)).to_string_lossy().into_owned();
            ffi::lua_settop(l, 0);
            let mine = [v[0], v[1], v[2], v[3], v[4], v[5], dl, gl];
            let lua: Vec<f64> = got.split(' ').map(parse_hex_float).collect();
            for k in 0..8 {
                assert!(
                    lua[k].to_bits() == mine[k].to_bits(),
                    "case {} value {}: lua {:e} native {:e}\ncur {:?}\ntg {:?}\ncom {:?} dt {} caps {} {} gain {}",
                    case, k, lua[k], mine[k], cur, tg, com, dt, cap_lin, cap_ang, gain
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 4000);
        ffi::lua_close(l);
    }
}

/// Parse C99 `%a` output ("0x1.8p+1", "-0x0p+0").
fn parse_hex_float(s: &str) -> f64 {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let s = s.trim_start_matches("0x");
    let (mant, exp) = s.split_once('p').expect("hex float");
    let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
    let mut m = u64::from_str_radix(int, 16).unwrap() as f64;
    let mut scale = 1.0 / 16.0;
    for ch in frac.chars() {
        m += ch.to_digit(16).unwrap() as f64 * scale;
        scale /= 16.0;
    }
    let e: i32 = exp.parse().unwrap();
    let v = m * 2f64.powi(e);
    if neg {
        -v
    } else {
        v
    }
}
