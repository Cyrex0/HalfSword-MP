//! Channel-0 keys (`SendMode::Latest`) and channel-1 supersede keys
//! (`SendMode::ReliableLatest`). A key is `stream << 24 | entity`, where the
//! entity is the source peer id (24 bits; peer ids stay below 2^24) or 0.
//! The sender keeps the newest unsent message per key; the receiver drops
//! a channel-0 message older than the newest it delivered for that key.

/// Build a key from a stream id and an entity (peer id or object group).
pub const fn key(stream: u8, entity: u32) -> u32 {
    (stream as u32) << 24 | (entity & 0x00FF_FFFF)
}

pub const fn stream_of(key: u32) -> u8 {
    (key >> 24) as u8
}

pub const fn entity_of(key: u32) -> u32 {
    key & 0x00FF_FFFF
}

// Channel-0 streams (legacy bodies moved unchanged).
/// `C2SRootStateTs` / `S2CSnapshotTs` (and the untimestamped forms).
pub const ROOT: u8 = 1;
/// `C2SSkeletalState` / `S2CSkeletalBroadcast` (pose frames).
pub const SKEL: u8 = 2;
/// `C2SWeaponStateTs` / `S2CWeaponBroadcastTs`.
pub const WEAPON: u8 = 3;
/// `C2SVitals` / `S2CVitals`.
pub const VITALS: u8 = 4;
/// `C2SWorldState` / `S2CWorldState` (entity = 0, one coalesced group).
pub const WORLD: u8 = 5;
/// `C2SPing` / `S2CPong`.
pub const PING: u8 = 6;
/// `C2SVoiceFrame` / `S2CVoiceBroadcast` (entity = peer, newest frame wins
/// only if the codec tolerates loss; voice stays best-effort).
pub const VOICE: u8 = 7;

// Channel-1 supersede keys.
pub const SESSION: u32 = key(0x80, 0);
pub const GAME_STATUS: u32 = key(0x81, 0);
/// `S2CPings` (needs `caps::PING`). 0x83..0x86 are used by v4 bodies.
pub const PINGS: u32 = key(0x87, 0);
/// v4 `S2CMatchState` while it coexists with `S2CSession`.
pub const MATCH_STATE: u32 = key(0x82, 0);
/// `S2CKit` per peer: `key(KIT, peer_id)`.
pub const KIT: u8 = 0x83;
/// `S2CKitRules`.
pub const KIT_RULES: u32 = key(0x84, 0);
/// `C2SLoadout`/`S2CLoadout` complete version per peer (reassembled by the
/// transport; the v4 manual chunking can go): `key(LOADOUT, peer_id)`.
pub const LOADOUT: u8 = 0x85;
/// `body` (the passport body for stand-ins, needs `caps::BODY`) per peer: `key(BODY, peer_id)`.
pub const BODY: u8 = 0x89;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pack_and_unpack() {
        let k = key(SKEL, 0x12_3456);
        assert_eq!((stream_of(k), entity_of(k)), (SKEL, 0x12_3456));
        assert_ne!(key(ROOT, 1), key(SKEL, 1));
        assert_ne!(SESSION, GAME_STATUS);
        assert_eq!(entity_of(key(ROOT, 0xFFFF_FFFF)), 0x00FF_FFFF);
    }
}
