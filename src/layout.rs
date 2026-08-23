//! Where the fields of a serialized structure sit.
//!
//! A cache is a memory image of fontconfig's own structs, so the offset of
//! every field depends on how the compiler laid those structs out. Two things
//! move them: the size of a pointer, and -- on 32-bit only -- whether a
//! `double` is aligned to one word or two. Fontconfig enumerates the result
//! in `fcarch.h` as six formats, and puts the name of the one it built for
//! into every cache file name:
//!
//! ```text
//! name      endianness   pointer   double alignment
//! le32d4    little       4         4
//! le32d8    little       4         8
//! le64      little       8         8
//! be32d4    big          4         4
//! be32d8    big          4         8
//! be64      big          8         8
//! ```
//!
//! This module derives all three of the little- or big-endian trio for
//! whichever target it is compiled for. Endianness itself is *not* handled:
//! see [`ARCHITECTURE`] for why reading a foreign-endian cache is not
//! something this crate attempts.
//!
//! # Trusting the arithmetic
//!
//! Every offset here is derived rather than written down, and the derivation
//! is a plain function of two numbers, so it can be checked for layouts this
//! machine cannot run. `fcarch.c` states five closed forms for the struct
//! sizes; the tests below check the derivation against all five, for every
//! pointer/alignment pair, and [`ASSERTIONS`] checks the live one at compile
//! time. A target whose layout is not one fontconfig knows fails to build.

/// The size of a serialized pointer, fontconfig's `SIZEOF_VOID_P`.
pub const PTR: usize = std::mem::size_of::<usize>();

/// The alignment of a `double`, fontconfig's `ALIGNOF_DOUBLE`.
///
/// On 64-bit targets this is 8 and changes nothing. On 32-bit it is the whole
/// difference between the `d4` and `d8` formats: i386 aligns a double to one
/// word, 32-bit ARM to two, and the same C structs come out different sizes.
pub const DOUBLE: usize = std::mem::align_of::<f64>();

/// The alignment of the 64-bit field in the cache header.
///
/// Fontconfig's own size formula for `FcCache` holds whether this is 4 or 8,
/// and on every target that matters it equals [`DOUBLE`]. It is named
/// separately so that the one place it is used says what it means.
const WIDE: usize = std::mem::align_of::<i64>();

/// The architecture tag fontconfig builds into a cache file name.
///
/// Reading a cache written for a different endianness is not attempted, and
/// the tag is why that costs nothing: a foreign-endian cache is a file we
/// never look for. Fontconfig on that machine wrote `be64` and we ask for
/// `le64`, so the two never meet -- and a build for a big-endian target asks
/// for `be64` and meets its own.
pub const ARCHITECTURE: &str = tag();

const fn tag() -> &'static str {
    let big = cfg!(target_endian = "big");
    match (big, PTR, DOUBLE) {
        (false, 8, _) => "le64",
        (false, 4, 4) => "le32d4",
        (false, 4, 8) => "le32d8",
        (true, 8, _) => "be64",
        (true, 4, 4) => "be32d4",
        (true, 4, 8) => "be32d8",
        // Fontconfig says the same thing in fcarch.h: a new shape needs a new
        // name, and guessing at one would name a file nothing else writes.
        _ => panic!("no fontconfig cache format for this pointer size and double alignment"),
    }
}

/// Round `offset` up to the next multiple of `align`.
const fn align_up(offset: usize, align: usize) -> usize {
    offset.div_ceil(align) * align
}

/// The larger of two numbers, since `usize::max` is not usable in a `const`.
const fn max(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// Every offset and size a serialized cache has, for one target shape.
///
/// Built by [`Layout::new`] from the two numbers that decide all of them, so
/// that a layout this machine cannot run can still be computed and checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `sizeof(FcAlign)`: what every serialized allocation is padded to.
    pub align: usize,

    // FcCache
    pub header: usize,
    pub magic: usize,
    pub version: usize,
    pub size: usize,
    pub dir: usize,
    pub dirs: usize,
    pub dirs_count: usize,
    pub set: usize,
    pub checksum: usize,
    pub checksum_nano: usize,

    // FcFontSet
    pub fontset: usize,
    pub nfont: usize,
    pub sfont: usize,
    pub fonts: usize,

    // FcPattern
    pub pattern: usize,
    pub num: usize,
    pub pattern_size: usize,
    pub elts: usize,
    pub pattern_ref: usize,

    // FcPatternElt
    pub elt: usize,
    pub object: usize,
    pub values: usize,

    // FcValueList
    pub node: usize,
    pub next: usize,
    pub node_value: usize,
    pub binding: usize,

    // FcValue
    pub value: usize,
    pub value_type: usize,
    pub union: usize,

    // FcCharSet
    pub charset: usize,
    pub charset_ref: usize,
    pub charset_num: usize,
    pub leaves: usize,
    pub numbers: usize,

    // FcLangSet, whose map is sized by the language table rather than here.
    pub langset_extra: usize,
    pub map_size: usize,
    pub map: usize,
}

impl Layout {
    /// The layout for a target with pointers of `ptr` bytes and doubles
    /// aligned to `dalign`.
    ///
    /// Fields are laid out in declaration order, each aligned to its own
    /// requirement, and the struct padded to its widest member -- which is
    /// all a C compiler does, and is why this can be arithmetic rather than a
    /// table.
    pub const fn new(ptr: usize, dalign: usize) -> Self {
        // The union inside an FcValue holds a double and a pointer, so it
        // starts wherever the wider of the two must.
        let union = max(ptr, dalign);
        let value_type = 0;
        let value = align_up(union + 8, union);

        // FcValueList { next, value, binding }
        let next = 0;
        let node_value = align_up(next + ptr, union);
        let binding = node_value + value;
        let node = align_up(binding + 4, union);

        // FcCache { magic, version, size, dir, dirs, dirs_count, set,
        //           checksum, checksum_nano }
        let magic = 0;
        let version = 4;
        let size = align_up(8, ptr);
        let dir = size + ptr;
        let dirs = dir + ptr;
        let dirs_count = dirs + ptr;
        let set = align_up(dirs_count + 4, ptr);
        let checksum = set + ptr;
        // The 64-bit field takes its own alignment, which every target
        // that matters gives the same as a double: see the assertion below.
        let checksum_nano = align_up(checksum + 4, dalign);
        let header = align_up(checksum_nano + 8, max(ptr, dalign));

        // FcFontSet { nfont, sfont, fonts }
        let nfont = 0;
        let sfont = 4;
        let fonts = align_up(8, ptr);
        let fontset = align_up(fonts + ptr, ptr);

        // FcPattern { num, size, elts_offset, ref }
        let num = 0;
        let pattern_size = 4;
        let elts = align_up(8, ptr);
        let pattern_ref = elts + ptr;
        let pattern = align_up(pattern_ref + 4, ptr);

        // FcPatternElt { object, values }
        let object = 0;
        let values = align_up(4, ptr);
        let elt = align_up(values + ptr, ptr);

        // FcCharSet { ref, num, leaves_offset, numbers_offset }
        let charset_ref = 0;
        let charset_num = 4;
        let leaves = align_up(8, ptr);
        let numbers = leaves + ptr;
        let charset = align_up(numbers + ptr, ptr);

        // FcLangSet { extra, map_size, map[] }
        let langset_extra = 0;
        let map_size = ptr;
        let map = map_size + 4;

        Self {
            // `FcAlign` is a union containing a double, so it is eight bytes
            // wide even where a double needs only four bytes of alignment.
            // Fontconfig asserts this outright.
            align: 8,
            header,
            magic,
            version,
            size,
            dir,
            dirs,
            dirs_count,
            set,
            checksum,
            checksum_nano,
            fontset,
            nfont,
            sfont,
            fonts,
            pattern,
            num,
            pattern_size,
            elts,
            pattern_ref,
            elt,
            object,
            values,
            node,
            next,
            node_value,
            binding,
            value,
            value_type,
            union,
            charset,
            charset_ref,
            charset_num,
            leaves,
            numbers,
            langset_extra,
            map_size,
            map,
        }
    }
}

/// The layout of the target this was compiled for.
pub const NATIVE: Layout = Layout::new(PTR, DOUBLE);

/// A leaf covers 256 codepoints as eight 32-bit words, on every target.
pub const LEAF: usize = 32;
/// `FcMatrix` is four doubles.
pub const MATRIX: usize = 32;
/// `FcRange` is two.
pub const RANGE: usize = 16;

/// Fontconfig's own size assertions, checked at compile time.
///
/// `fcarch.c` states these as closed forms, and a target where the derivation
/// above disagrees with them is one where every offset in this crate would be
/// wrong. Failing to build is the only acceptable outcome.
const ASSERTIONS: () = {
    assert!(NATIVE.value == 8 + max(PTR, DOUBLE), "sizeof(FcValue)");
    assert!(NATIVE.elt == 2 * PTR, "sizeof(FcPatternElt)");
    assert!(NATIVE.pattern == 8 + 2 * PTR, "sizeof(FcPattern)");
    assert!(NATIVE.charset == 8 + 2 * PTR, "sizeof(FcCharSet)");
    assert!(NATIVE.header == 16 + 6 * PTR, "sizeof(FcCache)");
    // A serialized offset has to fit the pointer it replaces, and the low bit
    // has to be free for fontconfig to tag it with.
    assert!(PTR == 4 || PTR == 8, "pointer size");
    // The header holds one 64-bit field, and the derivation aligns it the
    // way a double is aligned. Every target fontconfig names agrees, and one
    // that did not would put `checksum_nano` in the wrong place.
    assert!(WIDE == DOUBLE, "a 64-bit integer aligns differently to a double here");
    assert!(NATIVE.align.is_multiple_of(2), "offsets must stay even");
};

/// Force the assertions above to be evaluated.
const _: () = ASSERTIONS;

#[cfg(test)]
mod tests {
    use super::{Layout, ARCHITECTURE, DOUBLE, NATIVE, PTR};

    /// The shapes fontconfig has names for, as (pointer, double alignment).
    const SHAPES: [(usize, usize); 3] = [(4, 4), (4, 8), (8, 8)];

    /// The five closed forms `fcarch.c` asserts, for every shape -- including
    /// the two this machine cannot run.
    ///
    /// This is the whole reason the layout is a function rather than a table:
    /// the derivation for a 32-bit target can be checked on a 64-bit one.
    #[test]
    fn every_shape_matches_fontconfigs_own_formulas() {
        for (ptr, dalign) in SHAPES {
            let l = Layout::new(ptr, dalign);
            let m = if ptr > dalign { ptr } else { dalign };
            assert_eq!(l.value, 8 + m, "sizeof(FcValue) for {ptr}/{dalign}");
            assert_eq!(l.elt, 2 * ptr, "sizeof(FcPatternElt) for {ptr}/{dalign}");
            assert_eq!(l.pattern, 8 + 2 * ptr, "sizeof(FcPattern) for {ptr}/{dalign}");
            assert_eq!(l.charset, 8 + 2 * ptr, "sizeof(FcCharSet) for {ptr}/{dalign}");
            assert_eq!(l.header, 16 + 6 * ptr, "sizeof(FcCache) for {ptr}/{dalign}");
        }
    }

    /// The 64-bit numbers, which are the ones every parity harness has
    /// actually confirmed against fontconfig. If the derivation ever drifts,
    /// it drifts away from these.
    #[test]
    fn the_measured_layout_is_the_sixty_four_bit_one() {
        let l = Layout::new(8, 8);
        assert_eq!((l.magic, l.version, l.size), (0, 4, 8));
        assert_eq!((l.dir, l.dirs, l.dirs_count), (16, 24, 32));
        assert_eq!((l.set, l.checksum, l.checksum_nano), (40, 48, 56));
        assert_eq!(l.header, 64);
        assert_eq!((l.nfont, l.sfont, l.fonts, l.fontset), (0, 4, 8, 16));
        assert_eq!((l.num, l.pattern_size, l.elts, l.pattern_ref), (0, 4, 8, 16));
        assert_eq!((l.object, l.values, l.elt), (0, 8, 16));
        assert_eq!((l.next, l.node_value, l.binding, l.node), (0, 8, 24, 32));
        assert_eq!((l.value_type, l.union, l.value), (0, 8, 16));
        assert_eq!((l.charset_ref, l.charset_num, l.leaves, l.numbers), (0, 4, 8, 16));
        assert_eq!(l.charset, 24);
        assert_eq!((l.langset_extra, l.map_size, l.map), (0, 8, 12));
    }

    /// The 32-bit shapes differ from each other in exactly one place: where
    /// the union inside an `FcValue` starts, and everything downstream of it.
    #[test]
    fn the_two_thirty_two_bit_shapes_differ_only_in_the_value() {
        let d4 = Layout::new(4, 4);
        let d8 = Layout::new(4, 8);

        // The same, because nothing before the value depends on the double.
        assert_eq!(d4.header, d8.header);
        assert_eq!(d4.pattern, d8.pattern);
        assert_eq!(d4.elt, d8.elt);
        assert_eq!(d4.charset, d8.charset);

        // And different, which is the entire reason for two formats.
        assert_eq!((d4.union, d4.value, d4.node_value, d4.binding, d4.node), (4, 12, 4, 16, 20));
        assert_eq!((d8.union, d8.value, d8.node_value, d8.binding, d8.node), (8, 16, 8, 24, 32));
    }

    /// Every field has to sit inside the struct that holds it, which catches
    /// an alignment mistake that happens to keep the size right.
    #[test]
    fn no_field_runs_past_the_end_of_its_struct() {
        for (ptr, dalign) in SHAPES {
            let l = Layout::new(ptr, dalign);
            let at = |name: &str, offset: usize, width: usize, size: usize| {
                assert!(offset + width <= size, "{name} at {offset} in {size} ({ptr}/{dalign})");
            };
            at("checksum_nano", l.checksum_nano, 8, l.header);
            at("set", l.set, ptr, l.header);
            at("fonts", l.fonts, ptr, l.fontset);
            at("pattern_ref", l.pattern_ref, 4, l.pattern);
            at("values", l.values, ptr, l.elt);
            at("binding", l.binding, 4, l.node);
            at("union", l.union, 8, l.value);
            at("numbers", l.numbers, ptr, l.charset);
        }
    }

    /// The tag has to be one of the six fontconfig knows, and has to describe
    /// the layout actually compiled in.
    #[test]
    fn the_architecture_tag_names_this_layout() {
        assert!(
            ["le32d4", "le32d8", "le64", "be32d4", "be32d8", "be64"].contains(&ARCHITECTURE),
            "{ARCHITECTURE}"
        );
        assert_eq!(ARCHITECTURE.starts_with("le"), cfg!(target_endian = "little"));
        assert_eq!(ARCHITECTURE.contains("64"), PTR == 8);
        if PTR == 4 {
            assert_eq!(ARCHITECTURE.ends_with("d8"), DOUBLE == 8);
        }
        assert_eq!(NATIVE, Layout::new(PTR, DOUBLE));
    }
}
