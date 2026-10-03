//! Kit (class / gear selection), kit rules and the appearance loadout (kinds `0x05xx`, cap
//! `LOADOUT_KIT`), plus the two game-local bus keys of the Loadout mod (`kit_status`,
//! `standin_weapons`). ABI 2 / protocol v6: every contract below is one typed record, from
//! the game's slot through the sidecar and the server to the other games' slots.
//!
//! - `kit` (C2S, game slot `kit`): the player's selection. The sidecar resends the slot's
//!   bytes until a `kit_verdict` for us acks its `seq`.
//! - `kit_verdict` (S2C, `WireHdr::peer` = the owner; per-peer slot `peer_kit`): the server's
//!   validated kit (possibly a class default), the same layout as `kit`.
//! - `kit_rules_req` (C2S, game slot `kit_rules_req`, admin only) / `kit_rules` (S2C, slot
//!   `kit_rules`): the rules mode and budget.
//! - `loadout` (C2S, game blob `loadout`; S2C with `peer` = owner, per-peer blob
//!   `peer_loadout`): the worn appearance: both hand weapons' passports in the head, one row
//!   per armour piece / passport. Sent once per version (reliable, newest per owner); the
//!   transport fragments it (no application chunking).
//!
//! Item ids stay strings (`Str<32>`), not catalogue indices: the catalogue
//! (`server/src/loadout.rs` `catalog`, `HSMPLoadout/Scripts/hsmp_catalog.lua`) can change
//! between builds without a protocol bump, and an unknown string id is refused visibly
//! ("unknown armour 'x'") where a shifted index would silently mean another item. A kit is
//! a few hundred bytes, sent on change only.

use super::{flow, Chan, Dir, EnumInfo, RecordInfo, SlotForm, SlotInfo, CAP_BUS, CAP_LOADOUT_KIT};
use crate::layout::{Bool, Str};
use crate::record::Invalid;

/// Legacy (codec) kinds of this domain: none left (ABI 2).
pub const KINDS: &[super::KindInfo] = &[];

// ---- kit ------------------------------------------------------------------------------------

/// Armour ids per kit (one per slot group; the catalogue has 15 groups).
pub const KIT_MAX_ARMOR: usize = 16;

crate::ipc_pod! {
    /// One armour item id of a kit (row of `kit` / `kit_verdict`).
    pub struct KitItem {
        pub id: Str<32>,
    }

    /// A kit selection (`kit`, C2S) or the server's verdict on it (`kit_verdict`, S2C).
    pub struct Kit {
        /// `kit_verdict`: the server's revision (wall-clock ms based, monotonic). `kit`: 0.
        pub rev: u64,
        /// `kit`: the selection's sequence number. `kit_verdict`: the highest acked one.
        pub seq: u32,
        /// Armour rows.
        pub n: u16,
        /// `kit_verdict` code (`kit`: 0).
        pub verdict: u8,
        pub _r: u8,
        /// Cosmetics: face, hair colour, cloth tint, reserved (0 = game default).
        pub cos: [u8; 4],
        pub _r2: u32,
        /// Class id ("knight", ...), or "none" = keep the game's own gear (FREE mode only).
        pub class: Str<32>,
        /// Item id in the right / left hand ("" = empty).
        pub r: Str<32>,
        pub l: Str<32>,
        /// `kit_verdict`: why the selection was replaced ("" when accepted).
        pub reason: Str<96>,
    }

    /// Kit rules: the admin's request (`kit_rules_req`) or the server's rules (`kit_rules`).
    pub struct KitRules {
        /// `kit_rules`: the server's revision. `kit_rules_req`: 0.
        pub rev: u64,
        /// `kit_rules_req`: the request's sequence number (0 = no request). `kit_rules`: 0.
        pub seq: u32,
        pub budget: u16,
        /// `kit_mode` code.
        pub mode: u8,
        pub _r: u8,
    }
}

pub const MODE_FREE: u8 = 0;
pub const MODE_CLASSES: u8 = 1;
pub const MODE_CUSTOM: u8 = 2;

pub const VERDICT_ACCEPTED: u8 = 0;
pub const VERDICT_REPLACED: u8 = 1;
pub const VERDICT_DEFAULT: u8 = 2;

fn check_kit(k: &Kit) -> Result<(), Invalid> {
    if k.verdict > VERDICT_DEFAULT {
        return Err(Invalid::Range("verdict"));
    }
    Ok(())
}

fn check_rules(r: &KitRules) -> Result<(), Invalid> {
    if r.mode > MODE_CUSTOM {
        return Err(Invalid::Range("mode"));
    }
    Ok(())
}

pub const K_KIT: u16 = 0x0510;
pub const K_KIT_VERDICT: u16 = 0x0511;
pub const K_KIT_RULES_REQ: u16 = 0x0512;
pub const K_KIT_RULES: u16 = 0x0513;
pub const K_LOADOUT: u16 = 0x0514;
pub const K_KIT_STATUS: u16 = 0x0520;
pub const K_STANDIN_WEAPONS: u16 = 0x0521;

crate::record!(Kit, kind = K_KIT, name = "kit", rows = KitItem, count = n, max = KIT_MAX_ARMOR, check = check_kit);
crate::record!(KitRules, kind = K_KIT_RULES, name = "kit_rules", check = check_rules);

// ---- loadout (appearance) ---------------------------------------------------------------------

/// Class path bytes ("@Armor/..." short form or a full "/Game/..." path).
pub const CLASS_PATH: usize = 128;
/// Armour rows per loadout (pieces and passports, merged when they agree).
pub const LOADOUT_MAX_ROWS: usize = 48;
/// Bound of every scale / size / colour / price float (garbage beyond).
pub const LOADOUT_FLOAT_LIMIT: f32 = 1.0e6;

/// `LoadoutHead::flags`.
pub const LOADOUT_HAS_R: u8 = 1 << 0;
pub const LOADOUT_HAS_L: u8 = 1 << 1;
/// `ArmorRow::flags`: the row is a worn piece (slot + class) / an armour passport.
pub const ROW_PIECE: u8 = 1 << 0;
pub const ROW_PASSPORT: u8 = 1 << 1;

crate::ipc_pod! {
    /// A hand weapon's passport (Willie "Weapon Passport", HSMPLoadout WEAPON_FIELDS order).
    pub struct WeaponPass {
        /// WeaponClass.
        pub class: Str<128>,
        pub head_sub1: Str<128>,
        pub head_sub2: Str<128>,
        pub head: Str<128>,
        pub guard: Str<128>,
        pub pommel: Str<128>,
        pub grip: Str<128>,
        /// Name (an FName).
        pub name: Str<64>,
        pub id: i32,
        pub mat_steel: i32,
        pub mat_colored: i32,
        pub mat_wood: i32,
        pub mat_leather: i32,
        pub tier: i32,
        pub head_size: [f32; 3],
        pub guard_size: [f32; 3],
        pub grip_size: [f32; 3],
        pub pommel_size: [f32; 3],
        pub mass_head: f32,
        pub mass_guard: f32,
        pub mass_grip: f32,
        pub mass_pommel: f32,
        pub price: f32,
        pub color_wood: [f32; 4],
        pub color_leather: [f32; 4],
        pub _r: u32,
    }

    /// One armour row: a worn piece (`ROW_PIECE`: slot + class) and / or its passport
    /// (`ROW_PASSPORT`: every ARMOR_FIELDS field). A piece without a passport carries zeros.
    pub struct ArmorRow {
        /// ArmorCore class.
        pub class: Str<128>,
        pub id: i32,
        pub module1: i32,
        pub module2: i32,
        pub module3: i32,
        pub steel: i32,
        pub metal: i32,
        pub tier: i32,
        /// The passport's own Slot field.
        pub pslot: i32,
        pub price: f32,
        pub bg_color: [f32; 4],
        pub leather_color: [f32; 4],
        pub fabric1: [f32; 4],
        pub fabric2: [f32; 4],
        pub fabric3: [f32; 4],
        /// ArmorSlots_Enum key (piece slot / passport map key).
        pub slot: u8,
        /// `ROW_*` bits.
        pub flags: u8,
        pub core_removed: Bool,
        pub rust: Bool,
        pub dirt: Bool,
        pub up_ap: Bool,
        pub low_ap: Bool,
        pub req_up_ap: Bool,
        pub req_low_ap: Bool,
        pub req_hier: Bool,
        pub _r: [u8; 2],
    }

    /// Head of a `loadout` record: the version, both hands, then `n` armour rows.
    pub struct LoadoutHead {
        /// Owner's version (changes only when the gear does).
        pub version: u32,
        pub n: u16,
        /// `LOADOUT_HAS_*` bits.
        pub flags: u8,
        pub _r: u8,
        pub r: WeaponPass,
        pub l: WeaponPass,
    }
}

fn floats_ok(v: &[f32]) -> bool {
    v.iter().all(|x| x.abs() <= LOADOUT_FLOAT_LIMIT)
}

fn check_weapon(w: &WeaponPass, field: &'static str) -> Result<(), Invalid> {
    let all = [w.mass_head, w.mass_guard, w.mass_grip, w.mass_pommel, w.price];
    if !floats_ok(&all) || !floats_ok(&w.head_size) || !floats_ok(&w.guard_size) || !floats_ok(&w.grip_size)
        || !floats_ok(&w.pommel_size) || !floats_ok(&w.color_wood) || !floats_ok(&w.color_leather)
    {
        return Err(Invalid::Range(field));
    }
    if w.class.is_empty() {
        return Err(Invalid::Range(field));
    }
    Ok(())
}

fn check_loadout(h: &LoadoutHead) -> Result<(), Invalid> {
    if h.flags & !(LOADOUT_HAS_R | LOADOUT_HAS_L) != 0 {
        return Err(Invalid::Range("flags"));
    }
    if h.flags & LOADOUT_HAS_R != 0 {
        check_weapon(&h.r, "r")?;
    }
    if h.flags & LOADOUT_HAS_L != 0 {
        check_weapon(&h.l, "l")?;
    }
    Ok(())
}

fn check_armor_row(_h: &LoadoutHead, r: &ArmorRow) -> Result<(), Invalid> {
    if r.flags == 0 || r.flags & !(ROW_PIECE | ROW_PASSPORT) != 0 {
        return Err(Invalid::Range("flags"));
    }
    if r.class.is_empty() {
        return Err(Invalid::Range("class"));
    }
    if r.slot >= 64 {
        return Err(Invalid::Range("slot"));
    }
    if !floats_ok(&[r.price]) || !floats_ok(&r.bg_color) || !floats_ok(&r.leather_color) || !floats_ok(&r.fabric1)
        || !floats_ok(&r.fabric2) || !floats_ok(&r.fabric3)
    {
        return Err(Invalid::Range("color"));
    }
    Ok(())
}

crate::record!(LoadoutHead, kind = K_LOADOUT, name = "loadout", rows = ArmorRow, count = n, max = LOADOUT_MAX_ROWS,
    check = check_loadout, check_row = check_armor_row);

/// Whole-record rules a row check cannot see: at most one passport per slot.
pub fn check_loadout_rows(rows: &[ArmorRow]) -> Result<(), Invalid> {
    let mut seen = 0u64;
    for r in rows {
        if r.flags & ROW_PASSPORT != 0 {
            let bit = 1u64 << (r.slot & 63);
            if seen & bit != 0 {
                return Err(Invalid::Range("slot"));
            }
            seen |= bit;
        }
    }
    Ok(())
}

// ---- game-local bus keys (HSMPLoadout) ---------------------------------------------------------

crate::ipc_pod! {
    /// Bus key `kit_status`: the own pawn's kit evidence (kit.lua writes after every verify;
    /// the Director's kit step and HSMPLoadout read it). `pawn == ""` = no status.
    pub struct KitStatus {
        /// Writer's os.clock() at the write (the Director's process clock).
        pub t: f64,
        /// The kit's server revision (0 = no kit).
        pub rev: u64,
        pub round: u32,
        pub armour_n: u16,
        pub exp_armour_n: u16,
        pub tries: u16,
        pub ok: Bool,
        pub stable: Bool,
        /// `hand_vis` codes.
        pub r_visible: u8,
        pub l_visible: u8,
        pub _r: [u8; 2],
        /// The pawn's FName.
        pub pawn: Str<64>,
        /// Class id of the kit ("none" without one).
        pub kit: Str<32>,
        /// Short class name of what each hand holds ("" = empty).
        pub r_class: Str<64>,
        pub l_class: Str<64>,
        pub error: Str<192>,
    }

    /// Bus key `standin_weapons`: bumped on every stand-in hand-weapon destroy (HSMPLoadout),
    /// so HSMPAvatars drops its cached weapon components at once.
    pub struct StandinWeapons {
        pub gen: u64,
    }
}

pub const HAND_VIS_UNKNOWN: u8 = 0;
pub const HAND_VIS_SHOWN: u8 = 1;
pub const HAND_VIS_HIDDEN: u8 = 2;

fn check_status(s: &KitStatus) -> Result<(), Invalid> {
    if s.r_visible > HAND_VIS_HIDDEN || s.l_visible > HAND_VIS_HIDDEN {
        return Err(Invalid::Range("visible"));
    }
    Ok(())
}

crate::record!(KitStatus, kind = K_KIT_STATUS, name = "kit_status", check = check_status);
crate::record!(StandinWeapons, kind = K_STANDIN_WEAPONS, name = "standin_weapons");

// ---- tables -------------------------------------------------------------------------------------

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[RecordInfo] = &[
    crate::record_info!(Kit, cap = CAP_LOADOUT_KIT, flow = flow::C2S | flow::G2S,
        chan = Chan::RelLatest(hsmp_net_keys::KIT), doc = "kit selection (resent by the sidecar until a kit_verdict acks seq)"),
    crate::record_info!(alias K_KIT_VERDICT, "kit_verdict", Kit, cap = CAP_LOADOUT_KIT, flow = flow::S2C | flow::S2G,
        chan = Chan::RelLatest(hsmp_net_keys::KIT), doc = "server-validated kit; peer = owner"),
    crate::record_info!(alias K_KIT_RULES_REQ, "kit_rules_req", KitRules, cap = CAP_LOADOUT_KIT, flow = flow::C2S | flow::G2S,
        chan = Chan::Ordered, doc = "admin's kit rules request (seq 0 = none)"),
    crate::record_info!(KitRules, cap = CAP_LOADOUT_KIT, flow = flow::S2C | flow::S2G,
        chan = Chan::RelLatest(hsmp_net_keys::KIT_RULES), doc = "server kit rules"),
    crate::record_info!(LoadoutHead, cap = CAP_LOADOUT_KIT, flow = flow::C2S | flow::S2C | flow::G2S | flow::S2G,
        chan = Chan::RelLatest(hsmp_net_keys::LOADOUT), doc = "worn appearance (hand passports + armour rows), once per version; peer = owner"),
    crate::record_info!(KitStatus, cap = CAP_BUS, flow = flow::LOCAL, chan = Chan::None,
        doc = "own pawn kit evidence (kit.lua -> Director, Loadout)"),
    crate::record_info!(StandinWeapons, cap = CAP_BUS, flow = flow::LOCAL, chan = Chan::None,
        doc = "stand-in hand-weapon destroy generation (Loadout -> Avatars)"),
];

/// The hsmp-net stream ids of this domain (`hsmp_net::proto_v5::keys`, which this crate
/// does not depend on; a test in the server pins them).
pub mod hsmp_net_keys {
    pub const KIT: u8 = 0x83;
    pub const KIT_RULES: u8 = 0x84;
    pub const LOADOUT: u8 = 0x85;
}

/// Named record slots of this domain.
pub const SLOTS: &[SlotInfo] = &[
    SlotInfo { name: "kit", kind: K_KIT, form: SlotForm::Slot, dir: Dir::GameToSidecar, cap: CAP_LOADOUT_KIT,
        world_scoped: false, doc: "the player's kit selection (Menu writes and reads back; the sidecar sends it)" },
    SlotInfo { name: "kit_rules_req", kind: K_KIT_RULES_REQ, form: SlotForm::Slot, dir: Dir::GameToSidecar,
        cap: CAP_LOADOUT_KIT, world_scoped: false, doc: "the host's rules request (seq 0 = none)" },
    SlotInfo { name: "loadout", kind: K_LOADOUT, form: SlotForm::Blob, dir: Dir::GameToSidecar, cap: CAP_LOADOUT_KIT,
        world_scoped: false, doc: "own worn appearance (HSMPLoadout writer)" },
    SlotInfo { name: "peer_kit", kind: K_KIT_VERDICT, form: SlotForm::PeerSlot, dir: Dir::SidecarToGame,
        cap: CAP_LOADOUT_KIT, world_scoped: false, doc: "server-validated kit per peer (own peer id included)" },
    SlotInfo { name: "kit_rules", kind: K_KIT_RULES, form: SlotForm::Slot, dir: Dir::SidecarToGame, cap: CAP_LOADOUT_KIT,
        world_scoped: false, doc: "the server's kit rules" },
    SlotInfo { name: "peer_loadout", kind: K_LOADOUT, form: SlotForm::PeerBlob, dir: Dir::SidecarToGame,
        cap: CAP_LOADOUT_KIT, world_scoped: false, doc: "a peer's worn appearance (single reader: HSMPLoadout)" },
    SlotInfo { name: "kit_status", kind: K_KIT_STATUS, form: SlotForm::Bus, dir: Dir::Local, cap: CAP_BUS,
        world_scoped: false, doc: "own pawn kit evidence" },
    SlotInfo { name: "standin_weapons", kind: K_STANDIN_WEAPONS, form: SlotForm::Bus, dir: Dir::Local, cap: CAP_BUS,
        world_scoped: false, doc: "stand-in weapon destroy generation" },
];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[EnumInfo] = &[
    EnumInfo { name: "kit_mode", values: &[("FREE", MODE_FREE as u32), ("CLASSES", MODE_CLASSES as u32), ("CUSTOM", MODE_CUSTOM as u32)] },
    EnumInfo { name: "kit_verdict", values: &[("ACCEPTED", VERDICT_ACCEPTED as u32), ("REPLACED", VERDICT_REPLACED as u32),
        ("DEFAULT", VERDICT_DEFAULT as u32)] },
    EnumInfo { name: "loadout_flag", values: &[("HAS_R", LOADOUT_HAS_R as u32), ("HAS_L", LOADOUT_HAS_L as u32)] },
    EnumInfo { name: "loadout_row", values: &[("PIECE", ROW_PIECE as u32), ("PASSPORT", ROW_PASSPORT as u32)] },
    EnumInfo { name: "hand_vis", values: &[("UNKNOWN", HAND_VIS_UNKNOWN as u32), ("SHOWN", HAND_VIS_SHOWN as u32),
        ("HIDDEN", HAND_VIS_HIDDEN as u32)] },
];

/// Shared-memory homes of the records (segment fields).
pub type KitBuf = crate::record::VarBuf<Kit, KIT_MAX_ARMOR>;
pub type LoadoutBuf = crate::record::VarBuf<LoadoutHead, LOADOUT_MAX_ROWS>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{payload_len, to_payload, view};

    fn kit() -> (Kit, Vec<KitItem>) {
        let mut k = Kit::default();
        k.seq = 9;
        k.class = Str::new("knight");
        k.r = Str::new("w_longsword3");
        k.cos = [1, 2, 3, 0];
        (k, vec![KitItem { id: Str::new("h_armet") }, KitItem { id: Str::new("bv_1") }])
    }

    #[test]
    fn layouts_are_stable() {
        assert_eq!(core::mem::size_of::<Kit>(), 216);
        assert_eq!(core::mem::size_of::<KitItem>(), 32);
        assert_eq!(core::mem::size_of::<KitRules>(), 16);
        assert_eq!(core::mem::size_of::<WeaponPass>(), 1088);
        assert_eq!(core::mem::size_of::<ArmorRow>(), 256);
        assert_eq!(core::mem::size_of::<LoadoutHead>(), 2184);
        assert_eq!(core::mem::size_of::<KitStatus>(), 448);
        assert_eq!(core::mem::size_of::<StandinWeapons>(), 8);
    }

    #[test]
    fn kit_round_trip_and_hostile_bytes() {
        let (k, rows) = kit();
        let p = to_payload(&k, &rows);
        assert_eq!(p.len(), 216 + 64);
        let v = view::<Kit>(&p).unwrap();
        assert_eq!(v.head.class, "knight");
        assert_eq!(v.rows[1].id, "bv_1");
        assert!(super::super::check_payload(K_KIT_VERDICT, &p).is_ok(), "alias kind shares the layout");
        let mut bad = k;
        bad.verdict = 3;
        assert_eq!(view::<Kit>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("verdict"));
        let mut over = to_payload(&k, &[]);
        over[12] = 17; // n = 17 > 16
        assert!(matches!(view::<Kit>(&over), Err(Invalid::Rows { .. })));
        let mut s = p.clone();
        s[24 + 10] = 0xff; // invalid UTF-8 inside class
        assert!(matches!(view::<Kit>(&s), Err(Invalid::Str(_))));
        assert!(view::<Kit>(&p[..p.len() - 1]).is_err());
    }

    #[test]
    fn rules_range() {
        let r = KitRules { rev: 5, seq: 0, budget: 30, mode: MODE_CUSTOM, _r: 0 };
        assert_eq!(view::<KitRules>(&to_payload(&r, &[])).unwrap().head(), r);
        let bad = KitRules { mode: 3, ..r };
        assert_eq!(view::<KitRules>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Range("mode"));
    }

    fn weapon(class: &str) -> WeaponPass {
        let mut w = WeaponPass::default();
        w.class = Str::new(class);
        w.head_size = [1.0, 1.0, 1.0];
        w.color_wood = [0.2, 0.1, 0.05, 1.0];
        w
    }

    fn row(slot: u8, flags: u8) -> ArmorRow {
        let mut r = ArmorRow::default();
        r.class = Str::new("@Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Head_Sallet_Visor_A_001");
        r.slot = slot;
        r.flags = flags;
        r.bg_color = [0.0, 0.0, 0.0, 1.0];
        r
    }

    #[test]
    fn loadout_round_trip_and_checks() {
        let mut h = LoadoutHead::default();
        h.version = 77;
        h.flags = LOADOUT_HAS_R;
        h.r = weapon("@Weapons/Blueprints/Built_Weapons/Swords/BP_Sword_Arming_3");
        let rows = vec![row(1, ROW_PIECE | ROW_PASSPORT), row(2, ROW_PIECE)];
        let p = to_payload(&h, &rows);
        let v = view::<LoadoutHead>(&p).unwrap();
        assert_eq!(v.head.version, 77);
        assert_eq!(v.rows.len(), 2);
        check_loadout_rows(&v.rows).unwrap();
        // A hand flagged but without a class / a garbage size.
        let mut bad = h;
        bad.flags = LOADOUT_HAS_R | LOADOUT_HAS_L;
        assert_eq!(view::<LoadoutHead>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("l"));
        let mut bad = h;
        bad.r.head_size[1] = 1.0e9;
        assert_eq!(view::<LoadoutHead>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("r"));
        let mut bad = h;
        bad.flags = 4;
        assert_eq!(view::<LoadoutHead>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("flags"));
        // Row checks.
        assert_eq!(view::<LoadoutHead>(&to_payload(&h, &[row(1, 0)])).unwrap_err(), Invalid::Range("flags"));
        assert_eq!(view::<LoadoutHead>(&to_payload(&h, &[row(64, ROW_PIECE)])).unwrap_err(), Invalid::Range("slot"));
        let mut nan = row(1, ROW_PASSPORT);
        nan.fabric2[0] = f32::NAN;
        assert!(matches!(view::<LoadoutHead>(&to_payload(&h, &[nan])), Err(Invalid::Float(_))));
        let mut empty = row(1, ROW_PIECE);
        empty.class = Str::default();
        assert_eq!(view::<LoadoutHead>(&to_payload(&h, &[empty])).unwrap_err(), Invalid::Range("class"));
        let mut b = row(1, ROW_PASSPORT);
        b.rust = Bool(2);
        assert!(matches!(view::<LoadoutHead>(&to_payload(&h, &[b])), Err(Invalid::Bool(_))));
        // Duplicate passport slot.
        assert!(check_loadout_rows(&[row(3, ROW_PASSPORT), row(3, ROW_PASSPORT | ROW_PIECE)]).is_err());
        assert!(check_loadout_rows(&[row(3, ROW_PIECE), row(3, ROW_PIECE | ROW_PASSPORT)]).is_ok());
    }

    /// The largest loadout fits its shared-memory home, the ring-free reliable channel
    /// (one 64 KiB transport message with the 8-byte wire header) and a 16 KiB budget.
    #[test]
    fn largest_loadout_fits() {
        let max = payload_len::<LoadoutHead>(LOADOUT_MAX_ROWS);
        assert_eq!(max, 2184 + 48 * 256);
        assert!(max + crate::wire::HDR <= 64 * 1024);
        assert!(max <= 16 * 1024);
        assert_eq!(core::mem::size_of::<LoadoutBuf>(), max);
        let mut h = LoadoutHead::default();
        h.flags = LOADOUT_HAS_R | LOADOUT_HAS_L;
        h.r = weapon(&"x".repeat(200));
        h.l = weapon(&"é".repeat(100));
        assert_eq!(h.r.class.len(), 128);
        let rows: Vec<ArmorRow> = (0..LOADOUT_MAX_ROWS as u8).map(|i| row(i, ROW_PIECE | ROW_PASSPORT)).collect();
        let p = to_payload(&h, &rows);
        assert_eq!(p.len(), max);
        view::<LoadoutHead>(&p).unwrap();
        check_loadout_rows(&rows).unwrap();
        let mut buf = LoadoutBuf::new_boxed();
        buf.load(&p).unwrap();
        assert_eq!(buf.payload(), &p[..]);
    }

    #[test]
    fn status_and_weapon_gen() {
        let mut s = KitStatus::default();
        s.pawn = Str::new("Willie_BP_C_11");
        s.ok = Bool::TRUE;
        s.r_visible = HAND_VIS_SHOWN;
        assert!(view::<KitStatus>(&to_payload(&s, &[])).is_ok());
        s.l_visible = 3;
        assert_eq!(view::<KitStatus>(&to_payload(&s, &[])).unwrap_err(), Invalid::Range("visible"));
        let g = StandinWeapons { gen: 4 };
        assert_eq!(view::<StandinWeapons>(&to_payload(&g, &[])).unwrap().head(), g);
    }

    #[test]
    fn slots_and_records_resolve() {
        for s in SLOTS {
            assert!(super::super::record_info(s.kind).is_some(), "{}", s.name);
        }
        assert_eq!(super::super::record_by_name("kit_verdict").unwrap().layout, "Kit");
        assert_eq!(super::super::record_by_name("kit_rules_req").unwrap().layout, "KitRules");
    }
}
