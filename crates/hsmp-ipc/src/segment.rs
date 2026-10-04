//! The whole segment as one `#[repr(C)]` struct: offsets are compile-time
//! constants and every region starts on a 4 KiB boundary.

use crate::header::Header;
use crate::layout::IpcType;
use crate::ring::Ring;
use crate::schema::bus::{BusDir, BusValue, BUS_KEYS};
use crate::schema::pose::{PeerDir, PeerPlay, PeerRoot, PoseBuf, PoseLead, Root, Weapon, MAX_PEER_SLOTS};
use crate::schema::combat::Vitals;
use crate::schema::loadout::{KitBuf, KitRules, LoadoutBuf};
use crate::record::VarBuf;
use crate::schema::world as w;
use crate::schema::session::{AdminBuf, Link, SessionBuf};
use crate::schema::Stamped;
use crate::seqlock::SeqSlot;
use crate::triple::TripleBuf;

pub const G2S_SLOTS: usize = 512;
pub const S2G_SLOTS: usize = 1024;
pub const DEVCTL_SLOTS: usize = 64;

/// World record blobs (schema/world.rs): head + the full row capacity.
pub type WorldOutBuf = Stamped<VarBuf<w::WorldStateHead, { w::STATE_MAX }>>;
pub type WorldManifestBuf = Stamped<VarBuf<w::ManifestHead, { w::MANIFEST_MAX }>>;
/// My own dropped items (at most `server::world::MAX_DYN_PER_PEER`).
pub type WorldDynOutBuf = Stamped<VarBuf<w::DynHead, 32>>;
pub type WorldDynBuf = Stamped<VarBuf<w::DynHead, { w::DYN_MAX }>>;
pub type WorldHashBuf = Stamped<VarBuf<w::HashHead, { w::HASH_MAX }>>;
pub type WorldRemoteBuf = Stamped<VarBuf<w::RemoteHead, { w::REMOTE_MAX }>>;
pub type WorldOwnersBuf = Stamped<VarBuf<w::OwnersHead, { w::OWNERS_MAX }>>;
pub type WorldVerdictBuf = Stamped<VarBuf<w::VerdictHead, { w::VERDICT_MAX }>>;
pub type G2sRing = Ring<G2S_SLOTS>;
pub type S2gRing = Ring<S2G_SLOTS>;
pub type DevRing = Ring<DEVCTL_SLOTS>;

crate::ipc_layout! {
    /// Game-written latest-value slots.
    #[repr(align(4096))]
    pub struct GameOut {
        pub local_root: SeqSlot<Stamped<Root>>,
        pub local_weapon: SeqSlot<Stamped<Weapon>>,
        pub local_pose: SeqSlot<Stamped<PoseBuf>>,
        pub pose_lead: SeqSlot<PoseLead>,
        pub local_vitals: SeqSlot<Stamped<Vitals>>,
        pub kit: SeqSlot<Stamped<KitBuf>>,
        pub kit_rules_req: SeqSlot<Stamped<KitRules>>,
    }

    /// Game-written large blobs.
    #[repr(align(4096))]
    pub struct GameBlobs {
        pub world_out: TripleBuf<WorldOutBuf>,
        pub loadout: TripleBuf<Stamped<LoadoutBuf>>,
        pub world_manifest_out: TripleBuf<WorldManifestBuf>,
        pub world_dyn_out: TripleBuf<WorldDynOutBuf>,
        pub world_hash: TripleBuf<WorldHashBuf>,
    }

    /// Per-peer sidecar-written slots. `play` is written by the `hsmp-poseplay` thread,
    /// everything else by `hsmp-ipc`.
    pub struct PeerSlot {
        pub play: SeqSlot<PeerPlay>,
        pub root: SeqSlot<Stamped<PeerRoot>>,
        pub vitals: SeqSlot<Stamped<Vitals>>,
        pub kit: SeqSlot<Stamped<KitBuf>>,
    }

    #[repr(align(4096))]
    pub struct PeerTable {
        pub dir: SeqSlot<PeerDir>,
        /// The server's kit rules (sidecar-written; loadout domain).
        pub kit_rules: SeqSlot<Stamped<KitRules>>,
        pub slots: [PeerSlot; MAX_PEER_SLOTS],
    }

    #[repr(align(4096))]
    pub struct PeerLoadouts {
        pub slots: [TripleBuf<Stamped<LoadoutBuf>>; MAX_PEER_SLOTS],
    }

    /// Sidecar-written large blobs.
    #[repr(align(4096))]
    pub struct StateBlobs {
        /// Session domain record slots: the session snapshot, the sidecar's
        /// link view, the admin state.
        pub session: SeqSlot<Stamped<SessionBuf>>,
        pub link: SeqSlot<Stamped<Link>>,
        pub admin: SeqSlot<Stamped<AdminBuf>>,
        pub world_remote: TripleBuf<WorldRemoteBuf>,
        pub world_owners: TripleBuf<WorldOwnersBuf>,
        pub world_manifest: TripleBuf<WorldManifestBuf>,
        pub world_dyn: TripleBuf<WorldDynBuf>,
        pub world_consistency: SeqSlot<WorldVerdictBuf>,
    }

    /// Game-local bus.
    #[repr(align(4096))]
    pub struct Bus {
        pub dir: SeqSlot<BusDir>,
        pub keys: [SeqSlot<BusValue>; BUS_KEYS],
    }

    /// The whole shared segment.
    pub struct Segment {
        pub header: Header,
        pub game_out: GameOut,
        pub game_blobs: GameBlobs,
        pub peers: PeerTable,
        pub peer_loadouts: PeerLoadouts,
        pub state: StateBlobs,
        pub g2s: RingRegion<G2S_SLOTS>,
        pub s2g: RingRegion<S2G_SLOTS>,
        pub bus: Bus,
        pub devctl: RingRegion<DEVCTL_SLOTS>,
    }
}

/// Bytes in the segment (the mapping size).
pub const SEGMENT_SIZE: usize = core::mem::size_of::<Segment>();
const _: () = assert!(SEGMENT_SIZE.is_multiple_of(4096));
const _: () = assert!(SEGMENT_SIZE <= 8 * 1024 * 1024, "segment over the 8 MiB budget");

/// The layout hash both binaries carry.
pub const LAYOUT_HASH: u64 = crate::layout::layout_hash(&<Segment as IpcType>::DESC, crate::ABI_MAJOR);

/// Older layout hashes this build can still speak; empty: no older layout is accepted.
pub const COMPAT_HASHES: &[u64] = &[];

impl Segment {
    /// Initialise every triple buffer's control words. Run once by the creator (the game)
    /// on fresh zero pages, before publishing `init_state = READY`.
    pub fn init_blobs(&self) {
        let gb = &self.game_blobs;
        gb.world_out.init();
        gb.loadout.init();
        gb.world_manifest_out.init();
        gb.world_dyn_out.init();
        gb.world_hash.init();
        for s in &self.peer_loadouts.slots {
            s.init();
        }
        let st = &self.state;
        st.world_remote.init();
        st.world_owners.init();
        st.world_manifest.init();
        st.world_dyn.init();
    }

    /// A named record slot as bytes (`schema::SlotInfo`). `peer` is the peer-table
    /// slot for `PeerSlot` / `PeerBlob` forms (ignored otherwise). Bus keys are resolved by the
    /// game through `BusDir` (dynamic), not here. Domains add one arm per slot they declare.
    pub fn slot_ref(&self, name: &str, peer: usize) -> Option<&dyn crate::schema::RawSlot> {
        match name {
            "vitals" => Some(&self.game_out.local_vitals),
            "peer_vitals" => self.peers.slots.get(peer).map(|s| &s.vitals as &dyn crate::schema::RawSlot),
            // world (schema/world.rs)
            "world_out" => Some(&self.game_blobs.world_out),
            "world_manifest_out" => Some(&self.game_blobs.world_manifest_out),
            "world_dyn_out" => Some(&self.game_blobs.world_dyn_out),
            "world_hash" => Some(&self.game_blobs.world_hash),
            "world_remote" => Some(&self.state.world_remote),
            "world_owners" => Some(&self.state.world_owners),
            "world_manifest" => Some(&self.state.world_manifest),
            "world_dyn" => Some(&self.state.world_dyn),
            "world_consistency" => Some(&self.state.world_consistency),
            "session" => Some(&self.state.session),
            "link" => Some(&self.state.link),
            "admin" => Some(&self.state.admin),
            // loadout domain
            "kit" => Some(&self.game_out.kit),
            "kit_rules_req" => Some(&self.game_out.kit_rules_req),
            "loadout" => Some(&self.game_blobs.loadout),
            "kit_rules" => Some(&self.peers.kit_rules),
            "peer_kit" => self.peers.slots.get(peer).map(|s| &s.kit as &dyn crate::schema::RawSlot),
            "peer_loadout" => self.peer_loadouts.slots.get(peer).map(|s| s as &dyn crate::schema::RawSlot),
            // pose (schema/pose.rs SLOTS)
            "local_root" => Some(&self.game_out.local_root),
            "local_weapon" => Some(&self.game_out.local_weapon),
            "local_pose" => Some(&self.game_out.local_pose),
            "peer_root" => self.peers.slots.get(peer).map(|s| &s.root as &dyn crate::schema::RawSlot),
            _ => None,
        }
    }

    /// [`Segment::slot_ref`] by `SlotInfo`.
    pub fn slot(&self, info: &crate::schema::SlotInfo, peer: usize) -> Option<&dyn crate::schema::RawSlot> {
        self.slot_ref(info.name, peer)
    }

    #[inline]
    pub fn g2s(&self) -> &G2sRing {
        &self.g2s.ring
    }
    #[inline]
    pub fn s2g(&self) -> &S2gRing {
        &self.s2g.ring
    }
    #[inline]
    pub fn devctl(&self) -> &DevRing {
        &self.devctl.ring
    }
}

/// A ring on its own pages.
#[repr(C, align(4096))]
pub struct RingRegion<const N: usize> {
    pub ring: Ring<N>,
}

unsafe impl<const N: usize> IpcType for RingRegion<N> {
    const DESC: crate::layout::TypeDesc = crate::layout::TypeDesc::Struct {
        name: "RingRegion",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: false,
        fields: &[crate::layout::FieldDesc {
            name: "ring",
            offset: core::mem::offset_of!(Self, ring),
            ty: &<Ring<N> as IpcType>::DESC,
        }],
    };
}

const _: () = assert!(SEGMENT_SIZE > 3 * 1024 * 1024 && SEGMENT_SIZE < 6 * 1024 * 1024);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_page_aligned() {
        assert_eq!(core::mem::offset_of!(Segment, header), 0);
        for off in [
            core::mem::offset_of!(Segment, game_out),
            core::mem::offset_of!(Segment, game_blobs),
            core::mem::offset_of!(Segment, peers),
            core::mem::offset_of!(Segment, peer_loadouts),
            core::mem::offset_of!(Segment, state),
            core::mem::offset_of!(Segment, g2s),
            core::mem::offset_of!(Segment, s2g),
            core::mem::offset_of!(Segment, bus),
            core::mem::offset_of!(Segment, devctl),
        ] {
            assert_eq!(off % 4096, 0);
        }
        assert_eq!(crate::layout::layout_hash(&<Segment as IpcType>::DESC, crate::ABI_MAJOR), LAYOUT_HASH);
    }
}
