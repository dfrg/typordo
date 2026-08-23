//! Hashers for maps whose keys are small or already hashed.
//!
//! The standard library defaults to SipHash-1-3, which is the right default:
//! it is keyed per process, so a caller cannot feed a program keys that all
//! collide. Nothing here is exposed to that. The keys are language names,
//! file paths this process chose to look at, and hashes this crate computed
//! itself, so the protection buys nothing and the setup and finalisation
//! rounds cost more than the lookups they guard.
//!
//! Two hashers, for the two shapes that come up:
//!
//! * [`Fnv1a`] for short keys -- a few bytes to a few dozen. Its whole body
//!   is an xor and a multiply per byte, with nothing before or after.
//! * [`Passthrough`] for keys that are *already* a hash, where the only
//!   sensible thing is to use them as they are.
//!
//! Neither is suitable for a map whose keys come from outside the program,
//! and neither is any good on long keys, where SipHash processes eight bytes
//! at a time and wins.

use std::hash::{BuildHasherDefault, Hasher};

/// FNV-1a, for maps keyed by something short.
#[derive(Clone, Copy)]
pub struct Fnv1a(u64);

/// The offset basis and prime the algorithm specifies for 64 bits.
const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x100_0000_01b3;

impl Default for Fnv1a {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }
}

/// Build a map or set hashed with [`Fnv1a`].
pub type BuildFnv = BuildHasherDefault<Fnv1a>;

/// A hasher for keys that are already hashes.
///
/// Hashing a hash is work with nothing to show for it. The stored value is
/// used as it stands, which is sound only because the keys are produced by
/// something that already spread them -- see [`crate::casefold`].
#[derive(Clone, Copy, Default)]
pub struct Passthrough(u64);

impl Hasher for Passthrough {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    fn write(&mut self, bytes: &[u8]) {
        // Never reached while the only key type is `u64`. Mixing rather than
        // ignoring, so a future key type is merely slow and not broken.
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }
}

/// Build a map or set keyed by something already hashed.
pub type BuildPassthrough = BuildHasherDefault<Passthrough>;

#[cfg(test)]
mod tests {
    use super::{BuildFnv, BuildPassthrough, Fnv1a, OFFSET, PRIME};
    use std::collections::{HashMap, HashSet};
    use std::hash::Hasher;

    /// The published test vectors for FNV-1a, 64-bit. A hasher that is
    /// nearly right is worse than one that is obviously wrong.
    #[test]
    fn matches_the_published_vectors() {
        for (input, expected) in [
            ("", 0xcbf2_9ce4_8422_2325u64),
            ("a", 0xaf63_dc4c_8601_ec8c),
            ("foobar", 0x85944171f73967e8),
        ] {
            let mut hasher = Fnv1a::default();
            hasher.write(input.as_bytes());
            assert_eq!(hasher.finish(), expected, "{input:?}");
        }
    }

    #[test]
    fn the_constants_are_the_specified_ones() {
        assert_eq!(OFFSET, 14695981039346656037);
        assert_eq!(PRIME, 1099511628211);
    }

    /// Different inputs should mostly land in different buckets. Not a
    /// quality claim, just a check that nothing is degenerate.
    #[test]
    fn short_keys_spread() {
        let hashes: HashSet<u64> = (0..512u32)
            .map(|n| {
                let mut hasher = Fnv1a::default();
                hasher.write(format!("family {n}").as_bytes());
                hasher.finish()
            })
            .collect();
        assert_eq!(hashes.len(), 512);
    }

    /// Both hashers have to work as a `BuildHasher`, which is the only way
    /// they are ever used.
    #[test]
    fn both_drive_a_map() {
        let mut fnv: HashMap<String, u32, BuildFnv> = HashMap::default();
        for n in 0..100u32 {
            fnv.insert(format!("key {n}"), n);
        }
        assert_eq!(fnv.get("key 42"), Some(&42));
        assert_eq!(fnv.len(), 100);

        let mut through: HashMap<u64, u32, BuildPassthrough> = HashMap::default();
        for n in 0..100u64 {
            through.insert(n.wrapping_mul(PRIME), n as u32);
        }
        assert_eq!(through.get(&42u64.wrapping_mul(PRIME)), Some(&42));
        assert_eq!(through.len(), 100);
    }
}
