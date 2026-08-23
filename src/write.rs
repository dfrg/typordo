//! Writing a cache file.
//!
//! A cache is a memory image of fontconfig's own structures, so writing one
//! is laying out those structures in a buffer and replacing every pointer
//! with an offset. Reading it back is [`Cache`](crate::Cache); this is the
//! other half, and the two are tested against each other.
//!
//! # Layout, and why the order matters
//!
//! Fontconfig validates a cache before trusting it, and several of its
//! checks are about *where* things sit rather than what they say. Within one
//! pattern: the value list nodes of an element must run upwards in memory,
//! and a string, language set or range must sit at or after the element that
//! names it. Writing each pattern as a contiguous run -- the pattern struct,
//! its element array, then each element's values and their payloads --
//! satisfies all of that by construction, which is why the writer works in
//! that order rather than, say, pooling all the strings at the end.
//!
//! Character sets are the exception: fontconfig freezes them into a shared
//! table while serializing, so its own validator allows a charset to sit
//! anywhere. That is what lets this writer share one copy between every font
//! covering the same characters, and on a real font directory that sharing
//! is most of the file.

use std::collections::HashMap;

use crate::charset::{Coverage, LEAF_WORDS};
use crate::langset::Langs;
use crate::query::{OwnedValue, Query};
use crate::value::{Binding, Matrix, Range};

/// What every serialized structure is padded to, fontconfig's `FcAlign`.
const ALIGN: usize = 8;

/// `FcRef` for a structure that is not reference counted, `FC_REF_CONSTANT`.
///
/// Fontconfig refuses a cache whose patterns are not marked this way: a
/// pattern read out of a mapped file must never be freed.
const REF_CONSTANT: i32 = -1;

// Sizes of the structures, for a 64-bit little-endian build.
const HEADER: usize = 64;
const FONTSET: usize = 16;
const PATTERN: usize = 24;
const ELT: usize = 16;
const NODE: usize = 32;
const CHARSET: usize = 24;
const LEAF: usize = LEAF_WORDS * 4;
const MATRIX: usize = 32;
const RANGE: usize = 16;

// Field offsets, matching the reader's.
const H_MAGIC: usize = 0;
const H_VERSION: usize = 4;
const H_SIZE: usize = 8;
const H_DIR: usize = 16;
const H_DIRS: usize = 24;
const H_DIRS_COUNT: usize = 32;
const H_SET: usize = 40;
const H_CHECKSUM: usize = 48;
const H_CHECKSUM_NANO: usize = 56;

const FS_NFONT: usize = 0;
const FS_SFONT: usize = 4;
const FS_FONTS: usize = 8;

const P_NUM: usize = 0;
const P_SIZE: usize = 4;
const P_ELTS: usize = 8;
const P_REF: usize = 16;

const E_OBJECT: usize = 0;
const E_VALUES: usize = 8;

const N_NEXT: usize = 0;
const N_VALUE: usize = 8;
const N_BINDING: usize = 24;

const V_TYPE: usize = 0;
const V_UNION: usize = 8;

const CS_REF: usize = 0;
const CS_NUM: usize = 4;
const CS_LEAVES: usize = 8;
const CS_NUMBERS: usize = 16;

const LS_MAP_SIZE: usize = 8;
const LS_MAP: usize = 12;

/// A cache being assembled for one directory.
///
/// Everything is borrowed until [`finish`](CacheWriter::finish) runs, which
/// is the only point anything is copied.
///
/// ```no_run
/// # use fontconf::{CacheWriter, Query};
/// # let fonts: Vec<Query> = Vec::new();
/// let mut writer = CacheWriter::new("/usr/share/fonts/dejavu");
/// writer.mtime(1_700_000_000, 0);
/// for font in &fonts {
///     writer.font(font);
/// }
/// std::fs::write("dejavu.cache-9", writer.finish())?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct CacheWriter<'a> {
    dir: &'a str,
    subdirs: Vec<&'a str>,
    fonts: Vec<&'a Query>,
    seconds: i32,
    nanoseconds: i64,
}

impl<'a> CacheWriter<'a> {
    /// A cache for `dir`, with no fonts and no subdirectories yet.
    pub fn new(dir: &'a str) -> Self {
        Self { dir, subdirs: Vec::new(), fonts: Vec::new(), seconds: 0, nanoseconds: 0 }
    }

    /// Record a subdirectory of this one.
    ///
    /// Fontconfig does not flatten a tree into one cache: each directory
    /// gets its own, and this list is how a reader finds the rest.
    pub fn subdir(&mut self, path: &'a str) -> &mut Self {
        self.subdirs.push(path);
        self
    }

    /// The directory's last-modified time, in whole seconds and nanoseconds.
    ///
    /// This is what makes a cache stale: fontconfig compares it against the
    /// directory on disk and rescans if they differ. A cache written with
    /// the default of zero is treated as always current, which is useful in
    /// a test and wrong anywhere else.
    pub fn mtime(&mut self, seconds: i32, nanoseconds: i64) -> &mut Self {
        self.seconds = seconds;
        self.nanoseconds = nanoseconds;
        self
    }

    /// Add one font.
    ///
    /// One face can contribute several patterns; a variable font adds one
    /// per named instance.
    pub fn font(&mut self, pattern: &'a Query) -> &mut Self {
        self.fonts.push(pattern);
        self
    }

    /// Lay the whole cache out and return the bytes to write.
    pub fn finish(&self) -> Vec<u8> {
        let mut buf = Buffer::default();
        let header = buf.reserve(HEADER);

        let dir = buf.string(self.dir);
        buf.plain(header + H_DIR, header, dir);

        let dirs = buf.reserve(self.subdirs.len() * 8);
        buf.plain(header + H_DIRS, header, dirs);
        buf.i32(header + H_DIRS_COUNT, self.subdirs.len() as i32);
        for (index, path) in self.subdirs.iter().enumerate() {
            let at = buf.string(path);
            // Relative to the array, not to the slot: `FcCacheSubdir`.
            buf.plain(dirs + index * 8, dirs, at);
        }

        let set = buf.reserve(FONTSET);
        buf.plain(header + H_SET, header, set);
        buf.i32(set + FS_NFONT, self.fonts.len() as i32);
        buf.i32(set + FS_SFONT, self.fonts.len() as i32);
        let array = buf.reserve(self.fonts.len() * 8);
        buf.encoded(set + FS_FONTS, set, array);

        let mut charsets = CharSets::new();
        for (index, font) in self.fonts.iter().enumerate() {
            let at = pattern(&mut buf, font, &mut charsets);
            // Relative to the font set, not to the array: `FcFontSetFont`.
            buf.encoded(array + index * 8, set, at);
        }

        buf.u32(header + H_MAGIC, crate::cache::MAGIC_MMAP);
        buf.i32(header + H_VERSION, crate::cache::VERSION);
        buf.i64(header + H_SIZE, buf.bytes.len() as i64);
        buf.i32(header + H_CHECKSUM, self.seconds);
        buf.i64(header + H_CHECKSUM_NANO, self.nanoseconds);
        buf.bytes
    }
}

/// Serialized charsets, keyed by their contents so identical coverage is
/// written once.
type CharSets = HashMap<Vec<u8>, usize>;

/// One pattern, as a contiguous run: the struct, its elements, then their
/// values. See the module docs for why that order is required.
///
/// Properties a configuration invented are skipped. Fontconfig gives those
/// ids it mints at runtime, which mean nothing to the next process to read
/// the file, so writing them would record noise.
fn pattern(buf: &mut Buffer, query: &Query, charsets: &mut CharSets) -> usize {
    let count = query.len();
    let at = buf.reserve(PATTERN);
    buf.i32(at + P_NUM, count as i32);
    buf.i32(at + P_SIZE, count as i32);
    buf.i32(at + P_REF, REF_CONSTANT);

    let elts = buf.reserve(count * ELT);
    buf.plain(at + P_ELTS, at, elts);

    for (index, element) in query.elements().enumerate() {
        let elt = elts + index * ELT;
        buf.i32(elt + E_OBJECT, element.object().id());
        let mut previous: Option<usize> = None;
        for (value, binding) in element.values() {
            let node = buf.reserve(NODE);
            match previous {
                Some(before) => buf.encoded(before + N_NEXT, before, node),
                // Relative to the element, not to the array: `FcPatternElt`.
                None => buf.encoded(elt + E_VALUES, elt, node),
            }
            buf.i32(node + N_BINDING, code(binding));
            write_value(buf, node + N_VALUE, value, charsets);
            previous = Some(node);
        }
    }
    at
}

/// An `FcValue`: a tag, then either the value itself or an offset to it.
///
/// Offsets inside a value are relative to the value, not to the field
/// holding them.
fn write_value(buf: &mut Buffer, at: usize, value: &OwnedValue, charsets: &mut CharSets) {
    match value {
        OwnedValue::Void => buf.i32(at + V_TYPE, 0),
        OwnedValue::Int(v) => {
            buf.i32(at + V_TYPE, 1);
            buf.i32(at + V_UNION, *v);
        }
        OwnedValue::Double(v) => {
            buf.i32(at + V_TYPE, 2);
            buf.f64(at + V_UNION, *v);
        }
        OwnedValue::String(v) => {
            buf.i32(at + V_TYPE, 3);
            let text = buf.string(v);
            buf.encoded(at + V_UNION, at, text);
        }
        OwnedValue::Bool(v) => {
            buf.i32(at + V_TYPE, 4);
            buf.i32(at + V_UNION, i32::from(*v));
        }
        OwnedValue::Matrix(v) => {
            buf.i32(at + V_TYPE, 5);
            let matrix = write_matrix(buf, v);
            buf.encoded(at + V_UNION, at, matrix);
        }
        OwnedValue::CharSet(v) => {
            buf.i32(at + V_TYPE, 6);
            let set = write_charset(buf, v, charsets);
            buf.encoded(at + V_UNION, at, set);
        }
        // 7 is `FcTypeFTFace`, a live pointer that cannot be serialized.
        OwnedValue::LangSet(v) => {
            buf.i32(at + V_TYPE, 8);
            let set = write_langset(buf, v);
            buf.encoded(at + V_UNION, at, set);
        }
        OwnedValue::Range(v) => {
            buf.i32(at + V_TYPE, 9);
            let range = write_range(buf, v);
            buf.encoded(at + V_UNION, at, range);
        }
    }
}

fn write_matrix(buf: &mut Buffer, matrix: &Matrix) -> usize {
    let at = buf.reserve(MATRIX);
    buf.f64(at, matrix.xx);
    buf.f64(at + 8, matrix.xy);
    buf.f64(at + 16, matrix.yx);
    buf.f64(at + 24, matrix.yy);
    at
}

fn write_range(buf: &mut Buffer, range: &Range) -> usize {
    let at = buf.reserve(RANGE);
    buf.f64(at, range.begin);
    buf.f64(at + 8, range.end);
    at
}

/// A charset, shared with any earlier font covering the same characters.
///
/// The sharing is worth doing: a family with nine weights usually has nine
/// identical coverages, and a leaf costs 32 bytes per 256 codepoints.
fn write_charset(buf: &mut Buffer, coverage: &Coverage, charsets: &mut CharSets) -> usize {
    let leaves = coverage.leaves();
    let mut key = Vec::with_capacity(leaves.len() * (2 + LEAF));
    for (page, leaf) in &leaves {
        key.extend_from_slice(&page.to_le_bytes());
        for word in leaf {
            key.extend_from_slice(&word.to_le_bytes());
        }
    }
    if let Some(at) = charsets.get(&key) {
        return *at;
    }

    let at = buf.reserve(CHARSET);
    buf.i32(at + CS_REF, REF_CONSTANT);
    buf.i32(at + CS_NUM, leaves.len() as i32);
    let array = buf.reserve(leaves.len() * 8);
    buf.plain(at + CS_LEAVES, at, array);
    let numbers = buf.reserve(leaves.len() * 2);
    buf.plain(at + CS_NUMBERS, at, numbers);
    for (index, (page, leaf)) in leaves.iter().enumerate() {
        buf.u16(numbers + index * 2, *page);
        let bits = buf.reserve(LEAF);
        for (word, value) in leaf.iter().enumerate() {
            buf.u32(bits + word * 4, *value);
        }
        // Relative to the array, not to the slot: `FcCharSetLeaf`.
        buf.plain(array + index * 8, array, bits);
    }

    charsets.insert(key, at);
    at
}

/// A language set: a bitmap over fontconfig's language list.
///
/// The `extra` field holds languages that are not on that list, and is
/// deliberately left null -- fontconfig does not serialize it either, and
/// rejects a cache where it is anything else.
fn write_langset(buf: &mut Buffer, set: &Langs) -> usize {
    let words = set.words();
    let at = buf.reserve(LS_MAP + words.len() * 4);
    buf.u32(at + LS_MAP_SIZE, words.len() as u32);
    for (index, word) in words.iter().enumerate() {
        buf.u32(at + LS_MAP + index * 4, *word);
    }
    at
}

fn code(binding: Binding) -> i32 {
    match binding {
        Binding::Strong => 0,
        Binding::Weak => 1,
        Binding::Same => 2,
    }
}

/// The buffer being built, with the alignment invariant that makes offsets
/// encodable.
///
/// Every allocation is padded to eight bytes, so every offset is even and
/// the low bit is free for fontconfig to tag it with.
#[derive(Default)]
struct Buffer {
    bytes: Vec<u8>,
}

impl Buffer {
    /// Zeroed, aligned space for `len` bytes, at the end.
    fn reserve(&mut self, len: usize) -> usize {
        let at = self.bytes.len();
        debug_assert_eq!(at % ALIGN, 0, "the buffer should always stay aligned");
        self.bytes.resize(at + len.next_multiple_of(ALIGN), 0);
        at
    }

    /// A NUL-terminated string. The terminator is already zero.
    fn string(&mut self, text: &str) -> usize {
        let at = self.reserve(text.len() + 1);
        self.bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
        at
    }

    fn u16(&mut self, at: usize, value: u16) {
        self.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, at: usize, value: u32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn i32(&mut self, at: usize, value: i32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn i64(&mut self, at: usize, value: i64) {
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn f64(&mut self, at: usize, value: f64) {
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// An offset from `base` to `target`, stored plainly.
    fn plain(&mut self, at: usize, base: usize, target: usize) {
        self.i64(at, target as i64 - base as i64);
    }

    /// An offset from `base` to `target`, tagged as one.
    ///
    /// Fontconfig marks a relocated pointer by setting its low bit, so a
    /// field that was never relocated can be told apart from one that was.
    fn encoded(&mut self, at: usize, base: usize, target: usize) {
        self.i64(at, (target as i64 - base as i64) | 1);
    }
}

#[cfg(test)]
mod tests {
    use super::CacheWriter;
    use crate::{Binding, Cache, Coverage, Langs, Matrix, Object, OwnedValue, Query, Range};

    /// Write a cache and read it straight back, strictly.
    fn round_trip(writer: &CacheWriter<'_>) -> Cache {
        let bytes = writer.finish();
        let cache = Cache::new(bytes.into_boxed_slice()).expect("header");
        cache.validate().expect("structure");
        cache
    }

    #[test]
    fn an_empty_directory_round_trips() {
        let cache = round_trip(&CacheWriter::new("/usr/share/fonts/empty"));
        assert_eq!(cache.dir().unwrap(), "/usr/share/fonts/empty");
        assert_eq!(cache.subdirs().unwrap().len(), 0);
        assert_eq!(cache.fonts().unwrap().count(), 0);
    }

    #[test]
    fn subdirectories_round_trip() {
        let mut writer = CacheWriter::new("/fonts");
        writer.subdir("/fonts/truetype").subdir("/fonts/type1");
        let cache = round_trip(&writer);
        let subdirs: Vec<_> = cache.subdirs().unwrap().map(|d| d.unwrap()).collect();
        assert_eq!(subdirs, ["/fonts/truetype", "/fonts/type1"]);
    }

    #[test]
    fn the_directory_mtime_round_trips() {
        let mut writer = CacheWriter::new("/fonts");
        writer.mtime(1_700_000_000, 123_456_789);
        assert_eq!(round_trip(&writer).mtime().unwrap(), (1_700_000_000, 123_456_789));
    }

    /// One font carrying every value type the format has.
    fn kitchen_sink() -> Query {
        let mut coverage = Coverage::new();
        for c in ['A', 'B', 'Z', 'a', '\u{4e00}', '\u{10000}'] {
            coverage.insert(c);
        }
        let mut langs = Langs::new();
        langs.insert_index(crate::langs::index_of("en").unwrap());
        langs.insert_index(crate::langs::index_of("ja").unwrap());

        let mut font = Query::new();
        font.add(Object::File, "/fonts/Test.ttf");
        font.add(Object::Family, "Test");
        font.add(Object::Family, "Test Extra");
        font.add(Object::Index, 0);
        font.add(Object::Slant, 0);
        font.add(Object::Outline, true);
        font.add(Object::Scalable, false);
        font.add(Object::Size, OwnedValue::Double(12.5));
        font.add(Object::Weight, OwnedValue::Range(Range { begin: 40.0, end: 210.0 }));
        font.add(Object::Matrix, OwnedValue::Matrix(Matrix { xx: 1.0, xy: 0.2, yx: 0.0, yy: 1.0 }));
        font.add(Object::Charset, OwnedValue::CharSet(coverage));
        font.add(Object::Lang, OwnedValue::LangSet(langs));
        font.add(Object::Foundry, OwnedValue::Void);
        font
    }

    #[test]
    fn every_value_type_round_trips() {
        let font = kitchen_sink();
        let mut writer = CacheWriter::new("/fonts");
        writer.font(&font);
        let cache = round_trip(&writer);

        let read = cache.fonts().unwrap().next().expect("one font");
        assert_eq!(Query::from_pattern(&read), font);
    }

    #[test]
    fn bindings_round_trip() {
        let mut font = Query::new();
        font.add(Object::Family, "Strong");
        font.add_weak(Object::Family, "Weak");
        font.add_with_binding(Object::Family, "Same", Binding::Same);
        let mut writer = CacheWriter::new("/fonts");
        writer.font(&font);
        let cache = round_trip(&writer);

        let read = cache.fonts().unwrap().next().unwrap();
        let bindings: Vec<_> = read
            .get(Object::Family)
            .unwrap()
            .values()
            .bindings()
            .map(|(value, binding)| (value.as_str().unwrap().to_string(), binding))
            .collect();
        assert_eq!(
            bindings,
            [
                ("Strong".to_string(), Binding::Strong),
                ("Weak".to_string(), Binding::Weak),
                ("Same".to_string(), Binding::Same),
            ]
        );
    }

    /// Values keep the order they were added in: fontconfig treats the first
    /// family as the primary one, so a reordered chain is a different font.
    #[test]
    fn value_order_is_preserved() {
        let mut font = Query::new();
        for name in ["First", "Second", "Third", "Fourth"] {
            font.add(Object::Family, name);
        }
        let mut writer = CacheWriter::new("/fonts");
        writer.font(&font);
        let cache = round_trip(&writer);

        let read = cache.fonts().unwrap().next().unwrap();
        let names: Vec<_> =
            read.get(Object::Family).unwrap().values().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, ["First", "Second", "Third", "Fourth"]);
    }

    /// Fonts with the same coverage share one serialized charset. On a real
    /// directory that is most of the file, so it is worth asserting.
    #[test]
    fn identical_coverage_is_written_once() {
        let font = kitchen_sink();
        let mut one = CacheWriter::new("/fonts");
        one.font(&font);
        let mut many = CacheWriter::new("/fonts");
        for _ in 0..10 {
            many.font(&font);
        }

        let grew = many.finish().len() - one.finish().len();
        // Nine more patterns, but not nine more copies of the coverage.
        assert!(grew < 9 * 1024, "ten copies grew the cache by {grew} bytes");

        let cache = round_trip(&many);
        assert_eq!(cache.fonts().unwrap().count(), 10);
        for read in cache.fonts().unwrap() {
            assert_eq!(Query::from_pattern(&read), font);
        }
    }

    /// A font with no properties at all still has to produce a readable
    /// pattern: the element array is empty and the values pointer is null,
    /// which is the one case the offset encoding cannot express.
    #[test]
    fn an_empty_pattern_round_trips() {
        let font = Query::new();
        let mut writer = CacheWriter::new("/fonts");
        writer.font(&font);
        let cache = round_trip(&writer);
        let read = cache.fonts().unwrap().next().unwrap();
        assert!(read.is_empty());
    }

    /// The header records the file's own length, and fontconfig rejects a
    /// cache where it disagrees -- that is how a 32-bit cache is caught.
    #[test]
    fn the_recorded_size_is_the_real_size() {
        let font = kitchen_sink();
        let mut writer = CacheWriter::new("/fonts");
        writer.font(&font);
        let bytes = writer.finish();
        let declared = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
        assert_eq!(declared as usize, bytes.len());
        assert_eq!(bytes.len() % 8, 0);
    }
}
