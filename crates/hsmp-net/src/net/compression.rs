//! Independent lossless application blocks. Compression precedes transport
//! fragmentation/encryption and never depends on delivery of an earlier block.

const MAGIC: &[u8; 4] = b"HSC1";
const HEADER: usize = 12;
const MIN_INPUT: usize = 128;
const MIN_SAVING: usize = 32;

/// Return a self-contained LZ4 envelope only when its complete wire size saves
/// at least 32 bytes. Small or incompressible records retain their original kind.
pub fn pack(kind: u16, body: &[u8]) -> Option<Vec<u8>> {
    if kind == 0 || body.len() < MIN_INPUT || body.len() > u32::MAX as usize {
        return None;
    }
    let compressed = lz4_flex::block::compress(body);
    let encoded_len = HEADER.checked_add(compressed.len())?;
    if encoded_len.checked_add(MIN_SAVING)? > body.len() {
        return None;
    }
    let mut out = Vec::with_capacity(encoded_len);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[1, 0]); // codec=LZ4 block; reserved=0
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    Some(out)
}

/// The caller supplies the admitted message's maximum output size and checks
/// the returned kind against its native capability and message allow-list.
pub fn unpack(envelope: &[u8], max_output: usize) -> Result<(u16, Vec<u8>), &'static str> {
    if envelope.len() <= HEADER || &envelope[..4] != MAGIC {
        return Err("compression header");
    }
    let kind = u16::from_le_bytes([envelope[4], envelope[5]]);
    if kind == 0 || envelope[6] != 1 || envelope[7] != 0 {
        return Err("compression format");
    }
    let size = u32::from_le_bytes(envelope[8..12].try_into().map_err(|_| "compression length")?) as usize;
    if size < MIN_INPUT || size > max_output {
        return Err("compression output bound");
    }
    if envelope.len().checked_add(MIN_SAVING).is_none_or(|len| len > size) {
        return Err("compression saving");
    }
    let mut out = vec![0; size];
    let wrote = lz4_flex::block::decompress_into(&envelope[HEADER..], &mut out).map_err(|_| "compression block")?;
    if wrote != size {
        return Err("compression output length");
    }
    Ok((kind, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_native_bits_survive_independent_blocks() {
        let mut bytes = Vec::new();
        for _ in 0..256 {
            for value in [0u64, 0x8000_0000_0000_0000, 0x3ff0_0000_0000_0001, 0x7ff8_0000_0000_0042, u64::MAX] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let first = pack(0x0ad1, &bytes).expect("repeated exact values compress");
        let next = pack(0x0ad1, &bytes).expect("independent block compresses");
        assert_eq!(unpack(&next, bytes.len()), Ok((0x0ad1, bytes.clone())));
        assert_eq!(unpack(&first, bytes.len()), Ok((0x0ad1, bytes)));
    }

    #[test]
    fn small_or_incompressible_messages_stay_raw() {
        assert!(pack(1, &[0; 72]).is_none());
        let compact = [0u8; 317];
        let encoded = pack(0x0a91, &compact).expect("two-player state is eligible for compression");
        assert_eq!(unpack(&encoded, compact.len()), Ok((0x0a91, compact.to_vec())));
        let mut state = 0x7a35_61d1u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        assert!(pack(1, &noise).is_none());
        assert!(pack(0, &[0; 4096]).is_none());
    }

    #[test]
    fn decode_rejects_bounds_bad_headers_and_corrupt_blocks() {
        let encoded = pack(2, &[0x35; 4096]).unwrap();
        assert_eq!(unpack(&encoded, 4095), Err("compression output bound"));
        for offset in [0, 6, 7] {
            let mut bad = encoded.clone();
            bad[offset] ^= 0x80;
            assert!(unpack(&bad, 4096).is_err());
        }
        let mut bad = encoded.clone();
        bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(unpack(&bad, 4096), Err("compression output bound"));
        let mut bad = encoded.clone();
        bad[8..12].copy_from_slice(&4095u32.to_le_bytes());
        assert!(unpack(&bad, 4096).is_err());
        for end in [0, 4, HEADER, encoded.len() - 1] {
            assert!(unpack(&encoded[..end], 4096).is_err());
        }
    }
}
