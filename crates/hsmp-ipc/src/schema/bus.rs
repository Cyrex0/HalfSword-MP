//! Game-local bus (kinds `0x07xx`, cap `BUS`). Lua<->Lua contracts between the
//! mods' Lua states. Written and read only inside the game process, on the game thread. It
//! lives in the segment so `ipc-dump` and the tap can see it; the sidecar never reads it.
//!
//! Keys are named (`BusDir::names`, at most `BUS_KEYS`, each at most `BUS_KEY_BYTES` bytes).
//! Each key holds one typed record (a `SlotForm::Bus` slot of a domain's `SLOTS`) of at most
//! `BUS_VALUE_BYTES` in a seqlock slot ([`BusValue`]) whose `seq` is the key's generation.

use super::{Dir, Form, KindInfo, Stamped, CAP_BUS};

pub const BUS_KEYS: usize = 64;
pub const BUS_KEY_BYTES: usize = 32;
// Playback includes physical settlement evidence for all 32 peer slots.
pub const BUS_VALUE_BYTES: usize = 16384;

/// One bus key's slot body: the record payload behind its `SlotMeta` (like every record slot).
pub type BusValue = Stamped<[u8; BUS_VALUE_BYTES]>;

crate::ipc_pod! {
    /// Key names by bus slot (game-written).
    pub struct BusDir {
        pub count: u32,
        pub _r: u32,
        /// Bit i = key i is world-scoped (cleared at `world_leaving`).
        pub world_scoped: u64,
        pub names: [[u8; 32]; 64],
    }
}

/// Keys that are cleared at every world leave.
pub const WORLD_SCOPED_KEYS: &[&str] = &["puppets", "standin_dead", "playback", "pose_yield", "world_held", "spawn_status", "surrender_hold"];

pub const K_BUS: u16 = 0x0701;

pub const KINDS: &[KindInfo] = &[KindInfo {
    kind: K_BUS,
    name: "bus",
    cap: CAP_BUS,
    dir: Dir::Local,
    form: Form::Bus,
    replaces: ".playback.json .pose_yield.json .puppets.json .standin_weapons.gen .director.json .conn_state.json .spectate.json .spawn_status.json .spawn_request.json .kit_status.json .travel_request.json .travel_ack.json .ui_request.json .world_held.json .fallback_swap.json .return_to_lobby.flag",
}];

impl BusDir {
    /// Slot of `key`, if registered.
    pub fn find(&self, key: &str) -> Option<usize> {
        let k = key.as_bytes();
        if k.is_empty() || k.len() > BUS_KEY_BYTES {
            return None;
        }
        let n = (self.count as usize).min(BUS_KEYS);
        self.names[..n].iter().position(|nm| {
            let len = nm.iter().position(|&b| b == 0).unwrap_or(BUS_KEY_BYTES);
            &nm[..len] == k
        })
    }

    /// The name in slot `i` (lossy UTF-8).
    pub fn name(&self, i: usize) -> Option<String> {
        if i >= (self.count as usize).min(BUS_KEYS) {
            return None;
        }
        let nm = &self.names[i];
        let len = nm.iter().position(|&b| b == 0).unwrap_or(BUS_KEY_BYTES);
        Some(String::from_utf8_lossy(&nm[..len]).into_owned())
    }
}

// ---- ABI 2 records (protocol v6) ----------------------------------------------------------

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[super::RecordInfo] = &[];

/// Named record slots of this domain.
pub const SLOTS: &[super::SlotInfo] = &[];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[super::EnumInfo] = &[];
