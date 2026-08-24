//! Reading caches nobody wrote.
//!
//! The claim this crate makes loudest is that a corrupt cache yields an
//! `Error` and never a crash, and that claim is worth something only because
//! `/var/cache/fontconfig` is world-readable and any package installation can
//! rewrite it under a reader. Design alone does not settle it: every read
//! being bounds-checked stops a bad *slice*, and says nothing about the
//! arithmetic that computes where to slice.
//!
//! So these take real caches and damage them. `cargo test` builds with
//! overflow checks on, which is the point -- an offset computed by wrapping
//! is a wrong answer this crate tolerates on corrupt input, but it must not
//! be a panic, and here it would be one.
//!
//! Deterministic: the generator is a fixed-seed xorshift, so a failure names
//! a seed that reproduces it rather than a mutation nobody can find again.

use std::path::Path;

use typordo::{Cache, Object};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// xorshift64*, so a seed reproduces a failure exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Walk everything a caller could reach, so a bad offset is actually followed
/// rather than merely stored.
///
/// Every result is discarded: what is under test is that returning at all is
/// possible. The values are consumed rather than counted so that nothing is
/// optimised away, and the accumulator wraps -- a corrupt cache yields
/// nonsense numbers, and an overflow *here* would be the test failing rather
/// than the code under it.
fn walk(cache: &Cache) -> usize {
    let mut seen = 0usize;
    let _ = cache.dir();
    if let Ok(subdirs) = cache.subdirs() {
        for sub in subdirs {
            seen = seen.wrapping_add(sub.map_or(0, str::len));
        }
    }
    let Ok(fonts) = cache.fonts() else { return seen };
    for font in fonts {
        seen = seen.wrapping_add(font.len());
        let _ = font.validate();
        for element in font.elements() {
            seen = seen.wrapping_add(element.id() as usize);
            for value in element.values() {
                seen = seen.wrapping_add(match value {
                    typordo::ValueRef::String(s) => s.len(),
                    typordo::ValueRef::CharSet(chars) => {
                        let _ = chars.validate();
                        chars.chars().take(64).count() + chars.ranges().take(64).count()
                    }
                    typordo::ValueRef::LangSet(langs) => {
                        let _ = langs.validate();
                        langs.langs().take(64).count()
                    }
                    _ => 1,
                });
            }
        }
        // The accessors a caller actually reaches for.
        for object in [Object::Family, Object::File, Object::Charset, Object::Lang] {
            let _ = font.value(object);
            let _ = font.string(object);
        }
    }
    seen
}

/// Every byte of a real cache, one at a time, replaced by something else.
///
/// A single flipped byte is the mutation most likely to produce a *plausible*
/// structure -- a count slightly too large, an offset just past the end --
/// which is exactly the shape that gets past a header check and into the
/// arithmetic.
#[test]
fn a_single_corrupted_byte_never_panics() {
    let original = fixture("cantarell-le64.cache-9");
    let mut rng = Rng(0x5EED_1234_ABCD_0001);

    let mut opened = 0;
    for _ in 0..4000 {
        let mut bytes = original.clone();
        let at = rng.below(bytes.len());
        bytes[at] ^= 1 << rng.below(8);

        // A cache that fails its header check is the expected outcome for
        // most mutations; what matters is the ones that get past it.
        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            opened += 1;
            let _ = cache.validate();
            walk(&cache);
        }
    }
    // Not a statistic: if a change to the header check starts rejecting
    // these, the walk stops running and the test passes without testing.
    assert!(opened > 3000, "only {opened} of 4000 reached the read path");
}

/// Several bytes at once, which reaches structures a single flip cannot.
#[test]
fn many_corrupted_bytes_never_panic() {
    let mut opened = 0;
    let original = fixture("cantarell-le64.cache-9");
    let mut rng = Rng(0x5EED_1234_ABCD_0002);

    for _ in 0..2000 {
        let mut bytes = original.clone();
        for _ in 0..1 + rng.below(16) {
            let at = rng.below(bytes.len());
            bytes[at] = rng.next() as u8;
        }
        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            opened += 1;
            let _ = cache.validate();
            walk(&cache);
        }
    }
    assert!(opened > 1000, "only {opened} of 2000 reached the read path");
}

/// Offsets and counts are what the arithmetic runs on, so corrupt those
/// specifically rather than waiting for a random byte to land on one.
///
/// Whole aligned words are overwritten with values chosen to be awkward:
/// zero, one, the maximum, and values near it that overflow when scaled by a
/// stride or added to a base.
#[test]
fn hostile_counts_and_offsets_never_panic() {
    let mut opened = 0;
    let original = fixture("cantarell-le64.cache-9");
    let awkward: [u64; 10] = [
        0,
        1,
        u64::MAX,
        u64::MAX - 1,
        i64::MAX as u64,
        i64::MIN as u64,
        u32::MAX as u64,
        i32::MAX as u64,
        usize::MAX as u64 / 2,
        0x8000_0000_0000_0001,
    ];
    let mut rng = Rng(0x5EED_1234_ABCD_0003);

    for _ in 0..4000 {
        let mut bytes = original.clone();
        // Aligned, because that is where a count or an offset really sits.
        let word = rng.below(bytes.len() / 8) * 8;
        let value = awkward[rng.below(awkward.len())];
        bytes[word..word + 8].copy_from_slice(&value.to_ne_bytes());

        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            opened += 1;
            let _ = cache.validate();
            walk(&cache);
        }
    }
    assert!(opened > 3000, "only {opened} of 4000 reached the read path");
}

/// Truncation, which a byte-flipping generator never produces.
#[test]
fn a_truncated_cache_never_panics() {
    let original = fixture("cantarell-le64.cache-9");
    for len in 0..original.len().min(4096) {
        let bytes = original[..len].to_vec();
        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            let _ = cache.validate();
            walk(&cache);
        }
    }
    // And a few longer prefixes, spread over the rest of the file.
    let mut len = 4096;
    while len < original.len() {
        let bytes = original[..len].to_vec();
        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            let _ = cache.validate();
            walk(&cache);
        }
        len += 997;
    }
}

/// A valid header over a body of noise.
///
/// Purely random bytes never carry the right magic, so they stop at the
/// header and test nothing past it. Keeping the header intact is what puts
/// arbitrary content in front of the structure walker.
#[test]
fn a_valid_header_over_arbitrary_bytes_never_panics() {
    let original = fixture("cantarell-le64.cache-9");
    let mut rng = Rng(0x5EED_1234_ABCD_0004);
    let mut opened = 0;

    for _ in 0..2000 {
        let mut bytes = original.clone();
        // Everything after the header, which check_header has already
        // validated: magic, version, and the recorded length.
        for byte in bytes.iter_mut().skip(64) {
            *byte = rng.next() as u8;
        }
        if let Ok(cache) = Cache::new(bytes.into_boxed_slice()) {
            opened += 1;
            let _ = cache.validate();
            walk(&cache);
        }
    }
    assert!(opened > 1000, "only {opened} of 2000 reached the read path");
}
