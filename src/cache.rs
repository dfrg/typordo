use std::path::Path;

use crate::bytes::Bytes;
use crate::error::{Error, Result};
use crate::pattern::Pattern;

/// Magic on a cache written to disk, `FC_CACHE_MAGIC_MMAP`.
pub const MAGIC_MMAP: u32 = 0xFC02_FC04;
/// Magic on a cache built in memory, `FC_CACHE_MAGIC_ALLOC`.
///
/// It never appears in a file, and is rejected on read.
pub const MAGIC_ALLOC: u32 = 0xFC02_FC05;

/// The only cache format this crate reads.
///
/// Version 9 is what fontconfig 2.17 writes. The number is bumped whenever
/// the serialized layout changes, and a mismatch is not something to work
/// around: the file is a memory image, so a different version is a different
/// set of structures.
pub const VERSION: i32 = 9;

// Field offsets in `FcCache` for a 64-bit little-endian build. The header is
// exactly 64 bytes and the directory name follows immediately.
const MAGIC: usize = 0;
const VERSION_AT: usize = 4;
const SIZE: usize = 8;
const DIR: usize = 16;
const DIRS: usize = 24;
const DIRS_COUNT: usize = 32;
const SET: usize = 40;
const CHECKSUM: usize = 48;
const CHECKSUM_NANO: usize = 56;

/// `FcFontSet` is `nfont` (4), `sfont` (4), `fonts` (8).
const FS_NFONT: usize = 0;
const FS_FONTS: usize = 8;

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
    bytes: Box<[u8]>,
}

impl Cache {
    /// Read a cache file and validate its header.
    ///
    /// Only the header is checked here. Use [`Cache::validate`] to walk every
    /// pattern before trusting the contents.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::new(bytes.into_boxed_slice())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Validate an already-loaded cache file.
    pub fn new(bytes: Box<[u8]>) -> Result<Self> {
        let cache = Self { bytes };
        cache.check_header()?;
        Ok(cache)
    }

    fn data(&self) -> Bytes<'_> {
        Bytes::new(&self.bytes)
    }

    fn check_header(&self) -> Result<()> {
        let data = self.data();
        match data.u32(MAGIC)? {
            MAGIC_MMAP => {}
            other => return Err(Error::BadMagic(other)),
        }
        match data.i32(VERSION_AT)? {
            VERSION => {}
            other => return Err(Error::UnsupportedVersion(other)),
        }
        // The header's own length field is the strongest cheap check there
        // is: it is written as an `intptr_t`, so a cache from a 32-bit build
        // fails here rather than being misread as valid.
        let declared = data.i64(SIZE)?;
        if declared < 0 || declared as u64 != self.bytes.len() as u64 {
            return Err(Error::SizeMismatch {
                declared: declared as u64,
                actual: self.bytes.len(),
            });
        }
        // Prove the three top-level offsets land inside the file.
        data.resolve(0, data.i64(DIR)?)?;
        data.resolve(0, data.i64(DIRS)?)?;
        data.resolve(0, data.i64(SET)?)?;
        data.count(DIRS_COUNT)?;
        Ok(())
    }

    /// The directory this cache describes.
    pub fn dir(&self) -> Result<&str> {
        let data = self.data();
        data.str(data.resolve(0, data.i64(DIR)?)?)
    }

    /// The subdirectories fontconfig found beneath [`Cache::dir`].
    ///
    /// These are the directories a caller must load caches for in turn;
    /// fontconfig does not flatten a tree into one cache.
    pub fn subdirs(&self) -> Result<Subdirs<'_>> {
        let data = self.data();
        let base = data.resolve(0, data.i64(DIRS)?)?;
        let len = data.array(base, data.count(DIRS_COUNT)?, 8)?;
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
        Ok((data.i32(CHECKSUM)?, data.i64(CHECKSUM_NANO)?))
    }

    /// The fonts in this directory.
    ///
    /// One face can appear as several patterns: a variable font contributes
    /// one per named instance.
    pub fn fonts(&self) -> Result<Fonts<'_>> {
        let data = self.data();
        let set = data.resolve(0, data.i64(SET)?)?;
        let count = data.count(set + FS_NFONT)?;
        // The array of patterns is itself an encoded offset from the set. A
        // directory with no fonts stores a null array rather than an empty one.
        let Some(array) = data.follow(set, set + FS_FONTS)? else {
            return Ok(Fonts { data, set, array: 0, index: 0, len: 0 });
        };
        let len = data.array(array, count, 8)?;
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
        &self.bytes
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("dir", &self.dir().ok())
            .field("fonts", &self.fonts().map(|f| f.len).unwrap_or(0))
            .field("bytes", &self.bytes.len())
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
        let at = self.base + self.index * 8;
        self.index += 1;
        // Subdirectory offsets are relative to the start of the array, not to
        // their own slot: see `FcCacheSubdir` in `fcint.h`.
        Some(
            self.data
                .i64(at)
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
    fn pattern_at(&self, index: usize) -> Result<Pattern<'a>> {
        let slot = self.array + index * 8;
        // Pattern offsets are encoded relative to the font set, not to the
        // slot holding them: see `FcFontSetFont` in `fcint.h`.
        let at = self.data.follow(self.set, slot)?.ok_or(Error::NotAnOffset(0))?;
        Pattern::read(self.data, at)
    }
}

impl<'a> Iterator for Fonts<'a> {
    type Item = Pattern<'a>;

    fn next(&mut self) -> Option<Pattern<'a>> {
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
