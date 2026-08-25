use std::path::Path;

use crate::bytes::Bytes;
use crate::error::{Error, Result};
use crate::layout;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::pattern::PatternRef;
use crate::write::CacheWriter;

/// Magic on a cache written to disk, `FC_CACHE_MAGIC_MMAP`.
///
/// Fontconfig also has `FC_CACHE_MAGIC_ALLOC` (`0xFC02FC05`) for a cache it
/// built in memory. That one never reaches a file, so seeing it is an error.
pub(crate) const MAGIC_MMAP: u32 = 0xFC02_FC04;

/// The only cache format this crate reads.
///
/// Version 9 is what fontconfig 2.17 writes. The number is bumped whenever
/// the serialized layout changes, and a mismatch is not something to work
/// around: the file is a memory image, so a different version is a different
/// set of structures.
pub const VERSION: i32 = 9;

// Field offsets in `FcCache` and `FcFontSet`, for whichever shape this was
// built for: see [`crate::layout`].
use crate::layout::NATIVE as L;

/// One directory worth of scanned fonts, as fontconfig left it.
///
/// The whole file is held in memory and everything read out of it borrows,
/// so iterating fonts and reading their family names allocates nothing.
///
/// # What this does not do
///
/// A cache is a memory image of fontconfig's own structures, not a portable
/// format. It is tied to a format version, a word size and a byte order, all
/// of which fontconfig encodes in the file name it chooses
/// (`<hash>-le64.cache-9`). This reader handles 64-bit little-endian version
/// 9 and rejects everything else rather than guessing.
pub struct Cache {
    storage: Storage,
}

/// How a cache holds its bytes.
///
/// Reading copies the file; mapping does not. Everything above this is
/// written against `&[u8]` and does not care which it got.
enum Storage {
    /// Read into memory, which is the only way without the `mmap` feature.
    Owned(Box<[u8]>),
    /// Mapped, so that every process reading the same cache shares one copy.
    #[cfg(feature = "mmap")]
    Mapped(memmap2::Mmap),
}

impl Storage {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            #[cfg(feature = "mmap")]
            Self::Mapped(map) => map,
        }
    }
}

/// The size at which fontconfig switches from reading to mapping,
/// `FC_CACHE_MIN_MMAP`.
///
/// Below it the mapping costs more than the copy: a page of kernel bookkeeping
/// against a kilobyte of memcpy.
#[cfg(feature = "mmap")]
const MIN_MMAP: u64 = 1024;

impl Cache {
    /// Read a cache file and validate its header.
    ///
    /// Only the header is checked here. Use [`Cache::validate`] to walk every
    /// pattern before trusting the contents.
    ///
    /// With the `mmap` feature the file is mapped rather than read when it is
    /// large enough to be worth it, which is what makes one copy of a cache
    /// serve every process on the machine. See the feature's own
    /// documentation for what that costs.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        #[cfg(feature = "mmap")]
        {
            let file = std::fs::File::open(path)?;
            if file.metadata()?.len() >= MIN_MMAP {
                // SAFETY: there is none to claim, and this is the whole cost
                // of the feature. A cache is a file any process may rewrite,
                // so the bytes under the returned slice can change while it
                // is alive. Nothing in this crate reinterprets those bytes --
                // every field is read byte-wise through a bounds-checked
                // accessor -- so a change gives a wrong answer or an `Error`
                // rather than undefined behaviour here; the undefinedness is
                // in the aliasing itself. Fontconfig maps caches on the same
                // terms.
                #[allow(unsafe_code)]
                let map = unsafe { memmap2::Mmap::map(&file) }?;
                return Self::from_storage(Storage::Mapped(map));
            }
        }
        let bytes = std::fs::read(path)?;
        Self::from_storage(Storage::Owned(bytes.into_boxed_slice()))
    }

    /// A cache holding `fonts`, built in memory.
    ///
    /// What `FcConfigAppFontAddFile` is for: fonts an application ships
    /// rather than finds. Fontconfig keeps those in a second font set and
    /// walks `{ system, application }` in that order; this crate has no
    /// second set because matching takes an iterator of fonts, so an
    /// application font set is a cache you chain on:
    ///
    /// ```no_run
    /// # use typordo::{best, Cache, CachePolicy, Config, Pattern};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let (config, query): (Config, Pattern) = unimplemented!();
    /// # let scanned: Vec<Pattern> = unimplemented!();
    /// let app = Cache::from_fonts("/app/fonts", &scanned)?;
    /// // The caches have to outlive the iterator: a `PatternRef` borrows
    /// // from the one it came out of.
    /// let caches: Vec<_> = config.caches(CachePolicy::read_only()).collect();
    /// let system = caches.iter().flat_map(|(_, cache)| cache.fonts().into_iter().flatten());
    /// let chosen = best(&query, system.chain(app.fonts()?));
    /// # let _ = chosen; Ok(())
    /// # }
    /// ```
    ///
    /// Order decides a tie, and it is the caller's. Chaining the application
    /// first wins ties for it, which is the thing fontconfig cannot be asked
    /// for -- `FcFontSetMatchInternal` keeps its incumbent unless a font
    /// scores *strictly* better, and it sees the system's fonts first.
    ///
    /// The cache exists because a [`PatternRef`] is a cursor into cache
    /// bytes, and scanning produces owned [`Pattern`]s; this is the bridge,
    /// and it costs one pass over the patterns and no font parsing.
    ///
    /// `dir` is recorded as the directory the cache describes and is never
    /// read from. Nothing is written to disk. The recorded modification time
    /// is zero, which for a cache that is never checked against a directory
    /// means nothing at all -- but it would read as "never stale" if one were
    /// written out, so write it only with a stamp you meant.
    pub fn from_fonts<'a>(dir: &str, fonts: impl IntoIterator<Item = &'a Pattern>) -> Result<Self> {
        let mut writer = CacheWriter::new(dir);
        for font in fonts {
            writer.font(font);
        }
        Self::new(writer.finish().into_boxed_slice())
    }

    /// This cache with every path it holds moved under `dir`.    /// This cache with every path it holds moved under `dir`.
    ///
    /// A cache found for a directory it was not built for -- copied into an
    /// image, reached through a sysroot, or simply moved -- describes the
    /// machine that built it. `FcConfigAddCache` compares the directory a
    /// cache records with the one it was asked for and, when they differ,
    /// rebuilds each font's `FC_FILE` and each subdirectory as the requested
    /// directory plus the old basename. This is that, done once over the
    /// whole cache rather than per font at every read.
    ///
    /// Fontconfig rewrites the patterns in memory as it builds its font set;
    /// this crate hands out cursors into the mapped file, so there is nowhere
    /// to put a rewritten path. Rebuilding the cache image instead costs one
    /// pass and no font parsing, which is what makes it cheap enough to do on
    /// open -- and it needs no write access, so it still works on the
    /// read-only image that is the usual reason a cache is relocated at all.
    ///
    /// The recorded modification time is carried across unchanged. It has
    /// already been checked against the directory this cache is being used
    /// for; a relocation that preserved timestamps is exactly what let it
    /// match.
    ///
    /// One thing does not survive: properties the cache identifies only by a
    /// runtime id. Those ids were minted by whichever process wrote the file
    /// and mean nothing here, which is why [`Pattern::from_pattern`] drops
    /// them too.
    pub(crate) fn rebased(&self, dir: &str) -> Result<Self> {
        let (seconds, nanoseconds) = self.mtime()?;
        let subdirs: Vec<String> =
            self.subdirs()?.flatten().map(|path| rebase(dir, path)).collect();
        let fonts: Vec<Pattern> = self
            .fonts()?
            .map(|font| {
                let mut owned = Pattern::from_pattern(&font);
                if let Some(file) = font.string(Object::File) {
                    owned.set(Object::File, rebase(dir, file).as_str());
                }
                owned
            })
            .collect();

        let mut writer = CacheWriter::new(dir);
        writer.mtime(seconds, nanoseconds);
        for path in &subdirs {
            writer.subdir(path);
        }
        for font in &fonts {
            writer.font(font);
        }
        Self::new(writer.finish().into_boxed_slice())
    }

    /// Validate an already-loaded cache file.
    pub fn new(bytes: Box<[u8]>) -> Result<Self> {
        let cache = Self { storage: Storage::Owned(bytes) };
        cache.check_header()?;
        Ok(cache)
    }

    fn from_storage(storage: Storage) -> std::io::Result<Self> {
        let cache = Self { storage };
        match cache.check_header() {
            Ok(()) => Ok(cache),
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        }
    }

    fn data(&self) -> Bytes<'_> {
        Bytes::new(self.storage.as_bytes())
    }

    fn check_header(&self) -> Result<()> {
        let data = self.data();
        match data.u32(L.magic)? {
            MAGIC_MMAP => {}
            other => return Err(Error::BadMagic(other)),
        }
        match data.i32(L.version)? {
            VERSION => {}
            other => return Err(Error::UnsupportedVersion(other)),
        }
        // The header's own length field is the strongest cheap check there
        // is: it is written as an `intptr_t`, so a cache from a 32-bit build
        // fails here rather than being misread as valid.
        let declared = data.offset(L.size)?;
        if declared < 0 || declared as u64 != self.storage.as_bytes().len() as u64 {
            return Err(Error::SizeMismatch {
                declared: declared as u64,
                actual: self.storage.as_bytes().len(),
            });
        }
        // Prove the three top-level offsets land inside the file.
        data.resolve(0, data.offset(L.dir)?)?;
        data.resolve(0, data.offset(L.dirs)?)?;
        data.resolve(0, data.offset(L.set)?)?;
        data.count(L.dirs_count)?;
        Ok(())
    }

    /// The directory this cache describes.
    pub fn dir(&self) -> Result<&str> {
        let data = self.data();
        data.str(data.resolve(0, data.offset(L.dir)?)?)
    }

    /// The subdirectories fontconfig found beneath [`Cache::dir`].
    ///
    /// These are the directories a caller must load caches for in turn;
    /// fontconfig does not flatten a tree into one cache.
    pub fn subdirs(&self) -> Result<Subdirs<'_>> {
        let data = self.data();
        let base = data.resolve(0, data.offset(L.dirs)?)?;
        let len = data.array(base, data.count(L.dirs_count)?, layout::PTR)?;
        Ok(Subdirs { data, base, index: 0, len })
    }

    /// The last-modified time of the directory when the cache was written, as
    /// whole seconds and nanoseconds.
    ///
    /// Comparing this against the directory on disk is how fontconfig decides
    /// a cache is stale. Note the seconds field is a signed 32-bit value in
    /// this format version and overflows in 2038; fontconfig 2.18 widened it
    /// by claiming the padding word that follows.
    pub fn mtime(&self) -> Result<(i32, i64)> {
        let data = self.data();
        Ok((data.i32(L.checksum)?, data.i64(L.checksum_nano)?))
    }

    /// The fonts in this directory.
    ///
    /// One face can appear as several patterns: a variable font contributes
    /// one per named instance.
    pub fn fonts(&self) -> Result<Fonts<'_>> {
        let data = self.data();
        let set = data.resolve(0, data.offset(L.set)?)?;
        let count = data.count(set + L.nfont)?;
        // The array of patterns is itself an encoded offset from the set. A
        // directory with no fonts stores a null array rather than an empty one.
        let Some(array) = data.follow(set, set + L.fonts)? else {
            return Ok(Fonts { data, set, array: 0, index: 0, len: 0 });
        };
        let len = data.array(array, count, layout::PTR)?;
        Ok(Fonts { data, set, array, index: 0, len })
    }

    /// Walk every pattern, element and value, reporting the first problem.
    ///
    /// The iterators skip malformed entries so that one bad record does not
    /// hide the rest of a cache; this is the strict pass that turns
    /// corruption into an error instead.
    pub fn validate(&self) -> Result<()> {
        self.dir()?;
        for subdir in self.subdirs()? {
            subdir?;
        }
        let fonts = self.fonts()?;
        for index in 0..fonts.len {
            fonts.pattern_at(index)?.validate()?;
        }
        Ok(())
    }

    /// The raw file contents.
    pub fn as_bytes(&self) -> &[u8] {
        self.storage.as_bytes()
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("dir", &self.dir().ok())
            .field("fonts", &self.fonts().map(|f| f.len).unwrap_or(0))
            .field("bytes", &self.storage.as_bytes().len())
            .finish()
    }
}

/// Iterator over a cache's subdirectories.
pub struct Subdirs<'a> {
    data: Bytes<'a>,
    base: usize,
    index: usize,
    len: usize,
}

impl<'a> Iterator for Subdirs<'a> {
    type Item = Result<&'a str>;

    fn next(&mut self) -> Option<Result<&'a str>> {
        if self.index >= self.len {
            return None;
        }
        let at = self.base + self.index * layout::PTR;
        self.index += 1;
        // Subdirectory offsets are relative to the start of the array, not to
        // their own slot: see `FcCacheSubdir` in `fcint.h`.
        Some(
            self.data
                .offset(at)
                .and_then(|delta| self.data.resolve(self.base, delta))
                .and_then(|at| self.data.str(at)),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len - self.index;
        (n, Some(n))
    }
}

impl ExactSizeIterator for Subdirs<'_> {}

/// Iterator over the fonts in a cache.
#[derive(Clone)]
pub struct Fonts<'a> {
    data: Bytes<'a>,
    set: usize,
    array: usize,
    index: usize,
    len: usize,
}

impl<'a> Fonts<'a> {
    fn pattern_at(&self, index: usize) -> Result<PatternRef<'a>> {
        let slot = self.array + index * layout::PTR;
        // PatternRef offsets are encoded relative to the font set, not to the
        // slot holding them: see `FcFontSetFont` in `fcint.h`.
        let at = self.data.follow(self.set, slot)?.ok_or(Error::NotAnOffset(0))?;
        PatternRef::read(self.data, at)
    }
}

impl<'a> Iterator for Fonts<'a> {
    type Item = PatternRef<'a>;

    fn next(&mut self) -> Option<PatternRef<'a>> {
        while self.index < self.len {
            let index = self.index;
            self.index += 1;
            if let Ok(pattern) = self.pattern_at(index) {
                return Some(pattern);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.len - self.index))
    }
}

#[cfg(test)]
mod storage_tests {
    use super::Cache;

    /// However a cache is held, it has to read back the same.
    ///
    /// With the `mmap` feature this exercises both paths at once: the small
    /// cache is under the mapping threshold and gets read, the large one is
    /// over it and gets mapped.
    #[test]
    fn small_and_large_caches_read_alike() {
        let dir = std::env::temp_dir().join("typordo-storage");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for (name, fonts) in [("small", 0), ("large", 40)] {
            let font = {
                let mut font = crate::Pattern::new();
                font.add(crate::Object::File, "/fonts/Test.ttf");
                font.add(crate::Object::Family, "A Family With A Long Enough Name");
                font
            };
            let mut writer = crate::CacheWriter::new("/fonts");
            for _ in 0..fonts {
                writer.font(&font);
            }
            let bytes = writer.finish();
            let path = dir.join(name);
            std::fs::write(&path, &bytes).unwrap();

            let cache = Cache::open(&path).expect(name);
            cache.validate().expect(name);
            assert_eq!(cache.as_bytes(), bytes, "{name}");
            assert_eq!(cache.dir().unwrap(), "/fonts");
            assert_eq!(cache.fonts().unwrap().count(), fonts);
        }

        // The premise of the test: the two really are on opposite sides of
        // the threshold, or it proves nothing.
        assert!(std::fs::metadata(dir.join("small")).unwrap().len() < 1024);
        assert!(std::fs::metadata(dir.join("large")).unwrap().len() >= 1024);
    }

    /// A file too short to hold a header is rejected, not mapped and trusted.
    #[test]
    fn a_truncated_file_is_rejected() {
        let dir = std::env::temp_dir().join("typordo-storage-short");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stub");
        std::fs::write(&path, b"not a cache").unwrap();
        assert!(Cache::open(&path).is_err());
    }
}

/// `path` with its last component moved under `dir`.
///
/// `FcStrBuildFilename (forDir, FcStrBasename (path), NULL)`. A path with no
/// final component -- a root, or a trailing separator -- is left alone: there
/// is nothing to carry over and inventing one would name a different file.
///
/// Deliberately not `Path::join`, which uses the *host's* separator. A cache
/// holds the paths of the machine it describes, and this crate reads caches
/// for machines other than the one running it; joining a Unix path with a
/// backslash because Windows is doing the reading produces a path that names
/// nothing anywhere. The separator comes from `dir` instead, falling back to
/// `/` -- which every cache in the wild uses and which Windows accepts.
pub(crate) fn rebase(dir: &str, path: &str) -> String {
    let Some(name) = basename(path) else { return path.to_string() };
    let separator = if dir.contains('\\') && !dir.contains('/') { '\\' } else { '/' };
    let trimmed = dir.strip_suffix(['/', '\\']).unwrap_or(dir);
    let mut out = String::with_capacity(trimmed.len() + 1 + name.len());
    out.push_str(trimmed);
    out.push(separator);
    out.push_str(name);
    out
}

/// The last component of `path`, or `None` if it has none.
///
/// Both separators, whichever platform is reading: a cache written on Unix
/// holds `/` and one written on Windows holds `\`, and either may be read
/// from the other.
fn basename(path: &str) -> Option<&str> {
    let name = match path.rfind(['/', '\\']) {
        Some(at) => &path[at + 1..],
        None => path,
    };
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod rebase_tests {
    use super::{rebase, Cache};
    use std::path::Path;

    fn fixture() -> Cache {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cantarell-le64.cache-9");
        Cache::open(&path).expect("fixture cache")
    }

    /// Every property of every font, as text, so two caches can be compared
    /// without caring what types are involved.
    fn dump(cache: &Cache) -> Vec<Vec<(String, Vec<String>)>> {
        cache
            .fonts()
            .expect("fonts")
            .map(|font| {
                font.elements()
                    .map(|element| {
                        let name = match element.object() {
                            Some(object) => object.name().to_string(),
                            None => format!("#{}", element.id()),
                        };
                        (name, element.values().map(|v| format!("{v:?}")).collect())
                    })
                    .collect()
            })
            .collect()
    }

    /// Rebasing rebuilds the whole cache image, so every property of every
    /// font makes a round trip. The paths are the point; the risk is
    /// everything beside them. This asserts `file` is the only thing that
    /// moved -- compared field by field over a real cache of a variable font,
    /// which has six patterns and something in nearly every property.
    #[test]
    fn rebasing_moves_the_paths_and_nothing_else() {
        let cache = fixture();
        let old_dir = cache.dir().expect("a recorded directory").to_string();
        let new_dir = "/somewhere/else";

        let rebased = cache.rebased(new_dir).expect("rebased");
        assert_eq!(rebased.dir().unwrap(), new_dir);
        assert_eq!(rebased.mtime().unwrap(), cache.mtime().unwrap(), "the stamp is carried");

        let subdirs: Vec<&str> = cache.subdirs().unwrap().flatten().collect();
        let moved: Vec<&str> = rebased.subdirs().unwrap().flatten().collect();
        assert_eq!(subdirs.len(), moved.len());
        for (before, after) in subdirs.iter().zip(&moved) {
            assert_eq!(&rebase(new_dir, before), after);
        }

        let (before, after) = (dump(&cache), dump(&rebased));
        assert!(!before.is_empty(), "the fixture has fonts");
        assert_eq!(before.len(), after.len(), "same number of fonts");
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(b.len(), a.len(), "same properties, same count");
            for ((bo, bv), (ao, av)) in b.iter().zip(a) {
                assert_eq!(bo, ao, "same property, same order");
                let expected: Vec<String> =
                    bv.iter().map(|v| v.replace(&old_dir, new_dir)).collect();
                assert_eq!(&expected, av, "{bo} changed by more than the move");
            }
        }

        // And the move happened, or none of the above proves anything.
        let files: Vec<&String> = after
            .iter()
            .flatten()
            .filter(|(name, _)| name == "file")
            .flat_map(|(_, values)| values)
            .collect();
        assert!(!files.is_empty(), "the fonts have files");
        assert!(files.iter().all(|f| f.contains(new_dir)), "{files:?}");
    }

    /// Only the last component is carried over, and a path with none is left
    /// as it is rather than being turned into a different file.
    #[test]
    fn rebase_carries_the_basename() {
        assert_eq!(rebase("/new", "/old/dir/Font.ttf"), "/new/Font.ttf");
        // The separator follows the path being built, not the host: a Unix
        // cache read on Windows still has to come out with Unix paths.
        assert_eq!(rebase("/new", "C:\\old\\Font.ttf"), "/new/Font.ttf");
        assert_eq!(rebase("C:\\new", "/old/Font.ttf"), "C:\\new\\Font.ttf");
        // A trailing separator on the directory is not doubled.
        assert_eq!(rebase("/new/", "/old/Font.ttf"), "/new/Font.ttf");
        // Nothing to carry over leaves the path alone.
        assert_eq!(rebase("/new", "/"), "/");
        assert_eq!(rebase("/new", ""), "");
    }
}
