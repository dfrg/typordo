//! MD5, only because fontconfig names cache files after one.
//!
//! A cache lives at `<md5 of the directory path>-le64.cache-9`, so finding
//! the cache for a directory means reproducing that hash exactly. This is
//! content addressing, not security: nothing here depends on MD5 being hard
//! to invert, and no other part of the crate uses it.
//!
//! It is 60 lines and lets the crate keep its empty dependency list.

/// Per-round left-rotation amounts (RFC 1321, section 3.4).
#[rustfmt::skip]
const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// Round constants: `floor(2^32 * abs(sin(i + 1)))`.
#[rustfmt::skip]
const SINES: [u32; 64] = [
    0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, //
    0xf57c_0faf, 0x4787_c62a, 0xa830_4613, 0xfd46_9501, //
    0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be, //
    0x6b90_1122, 0xfd98_7193, 0xa679_438e, 0x49b4_0821, //
    0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa, //
    0xd62f_105d, 0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8, //
    0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed, //
    0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a, //
    0xfffa_3942, 0x8771_f681, 0x6d9d_6122, 0xfde5_380c, //
    0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70, //
    0x289b_7ec6, 0xeaa1_27fa, 0xd4ef_3085, 0x0488_1d05, //
    0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665, //
    0xf429_2244, 0x432a_ff97, 0xab94_23a7, 0xfc93_a039, //
    0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1, //
    0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1, //
    0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb, 0xeb86_d391,
];

/// The MD5 digest of `input`.
pub fn digest(input: &[u8]) -> [u8; 16] {
    let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

    // The message, padded with 0x80, zeroes, and its bit length as u64 LE.
    let mut tail = [0u8; 128];
    let rest = input.len() % 64;
    tail[..rest].copy_from_slice(&input[input.len() - rest..]);
    tail[rest] = 0x80;
    // The length field must land in the last 8 bytes of a 64-byte block, so
    // a remainder of 56 or more needs a second block to hold it.
    let tail_len = if rest < 56 { 64 } else { 128 };
    let bits = (input.len() as u64).wrapping_mul(8);
    tail[tail_len - 8..tail_len].copy_from_slice(&bits.to_le_bytes());

    let whole = input.len() - rest;
    let (blocks, _) = input[..whole].as_chunks::<64>();
    let (padding, _) = tail[..tail_len].as_chunks::<64>();
    for block in blocks.iter().chain(padding) {
        compress(&mut state, block);
    }

    let mut out = [0u8; 16];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (chunk, word) in chunks.iter_mut().zip(state) {
        *chunk = word.to_le_bytes();
    }
    out
}

/// The MD5 digest of `input` as lowercase hex, the form fontconfig uses in a
/// cache file name.
pub fn hex(input: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for byte in digest(input) {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap_or('0'));
    }
    out
}

fn compress(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    let (chunks, _) = block.as_chunks::<4>();
    for (word, chunk) in m.iter_mut().zip(chunks) {
        *word = u32::from_le_bytes(*chunk);
    }

    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (mix, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = mix.wrapping_add(a).wrapping_add(SINES[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(SHIFTS[i]));
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[cfg(test)]
mod tests {
    use super::hex;

    /// The test suite from RFC 1321, appendix A.5.
    #[test]
    fn rfc_1321_vectors() {
        for (input, expected) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890\
                 123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(hex(input.as_bytes()), expected, "md5({input:?})");
        }
    }

    /// Lengths either side of every padding boundary: a message that fills a
    /// block exactly, and one that leaves too little room for the length
    /// field and so needs a second padding block.
    #[test]
    fn padding_boundaries() {
        let long = "a".repeat(1000);
        for len in [54, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128] {
            assert_eq!(hex(&long.as_bytes()[..len]).len(), 32);
        }
        // The awkward lengths, each checked against coreutils `md5sum`: 55 is
        // the last that fits its length field in one block, 56 is the first
        // that needs a second, and 64 fills a block exactly.
        assert_eq!(hex(&long.as_bytes()[..55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(hex(&long.as_bytes()[..56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(hex(&long.as_bytes()[..64]), "014842d480b571495a4a0363793f7367");
    }
}
