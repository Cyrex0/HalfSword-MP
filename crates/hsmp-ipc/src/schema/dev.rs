//! Developer control (kinds `0x08xx`, cap `DEVCTL`). `hsmp-tools ipc-ctl` pushes
//! `dev_cmd` records into the segment's `DevCtl` ring: autotest commands, pose-tune knobs,
//! tdiag on/off. The mods read them with `dev_poll` (the native module fans them out to every
//! Lua state). Dev builds only.

use super::{KindInfo, CAP_DEVCTL};

/// No legacy (codec) kinds: the 0x0880 codec `devctl` kind was replaced by `dev_cmd`.
pub const KINDS: &[KindInfo] = &[];

// ---- ABI 2 records (protocol v6) ----------------------------------------------------------

crate::ipc_pod! {
    /// One developer command, pushed by `hsmp-tools ipc-ctl` into the game's `DevCtl` ring
    /// (dev builds; replaces `.autotest.cmd.jsonl`, `.pose_tune.json` and `.tdiag_on`).
    pub struct DevCmd {
        /// Tool-assigned id (echoed in the mod's log line).
        pub id: u32,
        /// `dev_op` code.
        pub op: u8,
        pub _r: [u8; 3],
        /// Numeric argument (a tune value, on/off).
        pub num: f64,
        /// Command / knob name ("join", "pose_stiffness", ...).
        pub key: crate::layout::Str<32>,
        /// Text argument (an autotest command's argument).
        pub arg: crate::layout::Str<192>,
    }
}

pub const K_DEV_CMD: u16 = 0x0810;

pub const DEV_OP_AUTOTEST: u8 = 1;
pub const DEV_OP_TUNE: u8 = 2;
pub const DEV_OP_TDIAG: u8 = 3;

fn check_dev(c: &DevCmd) -> Result<(), crate::record::Invalid> {
    if !(DEV_OP_AUTOTEST..=DEV_OP_TDIAG).contains(&c.op) {
        return Err(crate::record::Invalid::Range("op"));
    }
    Ok(())
}

crate::record!(DevCmd, kind = K_DEV_CMD, name = "dev_cmd", check = check_dev);

impl DevCmd {
    /// A command. `key` / `arg` are truncated on a UTF-8 boundary (32 / 192 bytes).
    pub fn new(id: u32, op: u8, key: &str, arg: &str, num: f64) -> DevCmd {
        let mut c = DevCmd::default();
        c.id = id;
        c.op = op;
        c.num = num;
        c.key.set(key);
        c.arg.set(arg);
        c
    }
}

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[super::RecordInfo] = &[crate::record_info!(DevCmd, cap = CAP_DEVCTL, flow = super::flow::LOCAL,
    chan = super::Chan::None, doc = "developer command (ipc-ctl -> DevCtl ring -> mods)")];

/// Named record slots of this domain.
pub const SLOTS: &[super::SlotInfo] = &[];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[super::EnumInfo] = &[super::EnumInfo {
    name: "dev_op",
    values: &[("AUTOTEST", DEV_OP_AUTOTEST as u32), ("TUNE", DEV_OP_TUNE as u32), ("TDIAG", DEV_OP_TDIAG as u32)],
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{view, Invalid};

    #[test]
    fn dev_cmd_round_trip() {
        let c = DevCmd::new(7, DEV_OP_TUNE, "pose_stiffness", "", 0.75);
        let b = bytemuck::bytes_of(&c).to_vec();
        assert_eq!(b.len(), 240);
        let v = view::<DevCmd>(&b).unwrap();
        let h = v.head();
        assert_eq!((h.id, h.op, h.num), (7, DEV_OP_TUNE, 0.75));
        assert_eq!(h.key, "pose_stiffness");
        assert!(crate::schema::check_payload(K_DEV_CMD, &b).is_ok());
    }

    #[test]
    fn dev_cmd_truncates_on_a_char_boundary() {
        let long = "é".repeat(200);
        let c = DevCmd::new(1, DEV_OP_AUTOTEST, &long, &long, 0.0);
        let h = view::<DevCmd>(bytemuck::bytes_of(&c)).unwrap().head();
        assert!(h.key.as_str().is_some() && h.key.len() <= 32 && h.key.len().is_multiple_of(2));
        assert!(h.arg.as_str().is_some() && h.arg.len() <= 192);
    }

    #[test]
    fn dev_cmd_hostile_bytes() {
        let ok = bytemuck::bytes_of(&DevCmd::new(1, DEV_OP_TDIAG, "tdiag", "", 1.0)).to_vec();
        // every op outside 1..=3
        for op in [0u8, 4, 0xff] {
            let mut b = ok.clone();
            b[4] = op;
            assert!(matches!(view::<DevCmd>(&b), Err(Invalid::Range("op"))), "op {op}");
        }
        // wrong sizes
        assert!(view::<DevCmd>(&ok[..239]).is_err());
        let mut long = ok.clone();
        long.push(0);
        assert!(view::<DevCmd>(&long).is_err());
        // non-finite num
        let mut b = ok.clone();
        b[8..16].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(view::<DevCmd>(&b).is_err());
        // non-canonical Str (bytes after the NUL, invalid UTF-8)
        let mut b = ok.clone();
        b[16 + 10] = b'x';
        assert!(view::<DevCmd>(&b).is_err());
        let mut b = ok.clone();
        b[48] = 0xff;
        assert!(view::<DevCmd>(&b).is_err());
        // arbitrary bytes never panic
        let mut x = 0x9e3779b97f4a7c15u64;
        for _ in 0..2000 {
            let mut b = ok.clone();
            for _ in 0..4 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let i = (x as usize) % b.len();
                b[i] = (x >> 32) as u8;
            }
            let _ = crate::schema::check_payload(K_DEV_CMD, &b);
        }
    }
}
