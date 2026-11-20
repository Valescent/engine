//! Pinned hashing for world identity and keyed randomness.
//!
//! Two hashes exist and neither may change once a world has shipped:
//!
//! - [`sha256`] over canonical description bytes. It is the world identity
//!   ([`crate::description::Description::description_hash`]) and the content
//!   digest. Identity is a 32-byte value, so the digest width is the contract.
//! - [`hash64`], a `SplitMix64` finaliser chain, for named streams and per-block
//!   randomness. It is integer-only, so it is exact on every target.
//!
//! Both are implemented here rather than pulled from a crate because the
//! project does not accept a new dependency without an ADR.

/// Initial state words of SHA-256 (FIPS 180-4).
const SHA256_IV: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// Round constants of SHA-256 (FIPS 180-4).
const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Returns the SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut state = SHA256_IV;
    let (blocks, remainder) = bytes.as_chunks::<64>();
    for block in blocks {
        compress(&mut state, block);
    }

    let mut tail = [0_u8; 128];
    tail[..remainder.len()].copy_from_slice(remainder);
    tail[remainder.len()] = 0x80;
    let tail_len = if remainder.len() < 56 { 64 } else { 128 };
    let bit_len = u64::try_from(bytes.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
    for block in tail[..tail_len].as_chunks::<64>().0 {
        compress(&mut state, block);
    }

    let mut digest = [0_u8; 32];
    for (word, out) in state.iter().zip(digest.as_chunks_mut::<4>().0) {
        *out = word.to_be_bytes();
    }
    digest
}

#[allow(clippy::many_single_char_names, clippy::similar_names)]
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut schedule = [0_u32; 64];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for i in 16..64 {
        let s0 = schedule[i - 15].rotate_right(7)
            ^ schedule[i - 15].rotate_right(18)
            ^ (schedule[i - 15] >> 3);
        let s1 = schedule[i - 2].rotate_right(17)
            ^ schedule[i - 2].rotate_right(19)
            ^ (schedule[i - 2] >> 10);
        schedule[i] = schedule[i - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (constant, word) in SHA256_K.iter().zip(schedule.iter()) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(*constant)
            .wrapping_add(*word);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

/// `SplitMix64` finaliser. Bijective on `u64`.
#[must_use]
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Converts a stream name into a stable domain tag.
///
/// Callers hold the tag and pass it to [`hash64`]; it never changes for a given
/// name, so a stream id is fixed forever.
#[must_use]
pub fn domain(name: &str) -> u64 {
    let digest = sha256(name.as_bytes());
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

/// Hashes a seed, a domain tag, and a coordinate word sequence.
///
/// Words are folded in order, so `hash64(s, d, &[x, z])` and
/// `hash64(s, d, &[z, x])` are different streams by construction.
#[must_use]
pub fn hash64(seed: u64, domain: u64, words: &[i64]) -> u64 {
    let mut hash = mix(seed ^ domain);
    for word in words {
        hash = mix(hash ^ word.cast_unsigned());
    }
    hash
}

/// Hashes a 2D coordinate pair.
#[must_use]
pub fn hash2(seed: u64, domain: u64, x: i32, z: i32) -> u64 {
    hash64(seed, domain, &[i64::from(x), i64::from(z)])
}

/// Hashes a 3D coordinate triple.
#[must_use]
pub fn hash3(seed: u64, domain: u64, x: i32, y: i32, z: i32) -> u64 {
    hash64(seed, domain, &[i64::from(x), i64::from(y), i64::from(z)])
}

/// Maps a hash into `[0, 1)` using the top 24 bits, which are exact in `f32`.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn hash01(hash: u64) -> f32 {
    let top = (hash >> 40) as u32;
    (top as f32) * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: [u8; 32]) -> String {
        use std::fmt::Write as _;

        digest.iter().fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
    }

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            hex(sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_handles_block_boundaries() {
        let cases = [
            (
                55,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
        ];
        for (len, expected) in cases {
            let input = vec![b'a'; len];
            assert_eq!(hex(sha256(&input)), expected, "len {len}");
        }
    }

    #[test]
    fn mix_is_bijective_over_a_sample() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..1024_u64 {
            assert!(seen.insert(mix(i)), "collision at {i}");
        }
    }

    #[test]
    fn stream_hash_is_order_sensitive() {
        let tag = domain("tree");
        assert_ne!(hash2(7, tag, 1, 2), hash2(7, tag, 2, 1));
        assert_ne!(hash64(7, tag, &[1]), hash64(8, tag, &[1]));
        assert_ne!(hash64(7, tag, &[1]), hash64(7, domain("ore"), &[1]));
    }

    #[test]
    fn stream_hash_is_stable() {
        let tag = domain("feat:ore");
        assert_eq!(hash3(42, tag, -3, 0, 9), hash3(42, tag, -3, 0, 9));
        assert_eq!(
            hash3(42, tag, -3, 0, 9),
            0xfe0a_5e5e_c6da_02a3,
            "pinned value; changing it changes every world"
        );
    }

    #[test]
    fn hash01_is_in_the_unit_interval() {
        let tag = domain("test");
        for i in 0..4096 {
            let value = hash01(hash2(1, tag, i, -i));
            assert!((0.0..1.0).contains(&value));
        }
        assert_eq!(hash01(u64::MAX), 1.0 - 1.0 / 16_777_216.0);
        assert_eq!(hash01(0), 0.0);
    }
}
