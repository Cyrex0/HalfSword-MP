//! Protocol v6 message framing: `[WireHdr: 8 bytes][record payload]`, one hsmp-net channel
//! message per record. The payload is the record's bytes exactly as they sit in shared memory
//! ([`crate::record`]). The sidecar frames a game record by prepending the header; the server
//! relays by patching `peer` (and `aux`) in place; receivers validate with [`decode`].
//!
//! Ring records ([`crate::ring::RecordHeader`]) carry the same three fields (`kind`, `aux`,
//! `peer`), so a message crosses between the network and shared memory as a copy.

use crate::record::{view, Invalid, Record, View};

/// Header bytes.
pub const HDR: usize = 8;

crate::ipc_pod! {
    /// Leads every v6 channel message.
    pub struct WireHdr {
        /// Record kind ([`Record::KIND`]).
        pub kind: u16,
        /// Kind-specific 16-bit field (e.g. a relayed pose frame's relay interval, ms).
        pub aux: u16,
        /// The peer a server->client message is about / from (0 client->server, 0 = server).
        pub peer: u32,
    }
}

/// Split a message into its header and payload.
#[inline]
pub fn split(msg: &[u8]) -> Result<(WireHdr, &[u8]), Invalid> {
    if msg.len() < HDR {
        return Err(Invalid::Header);
    }
    let (h, p) = msg.split_at(HDR);
    Ok((bytemuck::pod_read_unaligned::<WireHdr>(h), p))
}

/// The kind of a message without parsing the rest (0 if too short).
#[inline]
pub fn kind_of(msg: &[u8]) -> u16 {
    if msg.len() < 2 {
        0
    } else {
        u16::from_le_bytes([msg[0], msg[1]])
    }
}

/// Append a framed message: header + `payload`.
#[inline]
pub fn frame_payload(out: &mut Vec<u8>, kind: u16, aux: u16, peer: u32, payload: &[u8]) {
    out.reserve(HDR + payload.len());
    out.extend_from_slice(bytemuck::bytes_of(&WireHdr { kind, aux, peer }));
    out.extend_from_slice(payload);
}

/// A framed message as a new `Vec`.
pub fn message(kind: u16, aux: u16, peer: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HDR + payload.len());
    frame_payload(&mut v, kind, aux, peer, payload);
    v
}

/// Frame a record built in process.
pub fn encode<R: Record>(aux: u16, peer: u32, head: &R, rows: &[R::Row]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HDR + crate::record::payload_len::<R>(rows.len()));
    v.extend_from_slice(bytemuck::bytes_of(&WireHdr { kind: R::KIND, aux, peer }));
    crate::record::write_payload(&mut v, head, rows);
    v
}

/// Patch the `peer` field of a framed message in place (server relay).
#[inline]
pub fn set_peer(msg: &mut [u8], peer: u32) {
    if msg.len() >= HDR {
        msg[4..8].copy_from_slice(&peer.to_le_bytes());
    }
}

/// Patch the `aux` field of a framed message in place.
#[inline]
pub fn set_aux(msg: &mut [u8], aux: u16) {
    if msg.len() >= HDR {
        msg[2..4].copy_from_slice(&aux.to_le_bytes());
    }
}

/// Validate a framed message as an `R` (kind must match) and view it in place.
pub fn decode<R: Record>(msg: &[u8]) -> Result<(WireHdr, View<'_, R>), Invalid> {
    let (h, p) = split(msg)?;
    if h.kind != R::KIND {
        return Err(Invalid::Kind(h.kind));
    }
    Ok((h, view::<R>(p)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::ipc_pod! {
        pub struct Ping {
            pub t: u64,
        }
    }
    crate::record!(Ping, kind = 0x7f10, name = "t_ping");

    #[test]
    fn frame_patch_decode() {
        let mut m = encode(0, 0, &Ping { t: 42 }, &[]);
        assert_eq!(m.len(), 16);
        assert_eq!(kind_of(&m), 0x7f10);
        set_peer(&mut m, 9);
        set_aux(&mut m, 33);
        let (h, v) = decode::<Ping>(&m).unwrap();
        assert_eq!((h.kind, h.aux, h.peer, v.head.t), (0x7f10, 33, 9, 42));
        assert_eq!(decode::<Ping>(&m[..7]).unwrap_err(), Invalid::Header);
        let mut other = m.clone();
        other[0] = 1;
        assert!(matches!(decode::<Ping>(&other), Err(Invalid::Kind(_))));
    }
}
