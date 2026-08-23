//! Rebuilding caches: what `fc-cache` does.
//!
//! Scanning a directory is [`scan`](crate::scan_file) and writing the result
//! is [`CacheWriter`]. This is the part between them -- deciding whether the
//! cache on disk is still good, choosing where a new one goes, and replacing
//! it without anyone seeing a half-written file.
//!
//! # When a cache is stale
//!
//! Fontconfig records the directory's modification time in the cache and
//! compares it on every read. That is the whole test: not a hash of the
//! contents, not the font files' own timestamps. Adding or removing a file
//! changes the directory's mtime, so the cache is noticed; editing a font in
//! place without changing the directory does not, and the stale cache stands
//! until something forces a rescan.
//!
//! A directory whose mtime is zero is always taken as current, which is how a
//! read-only image ships caches that never expire.

use std::io;
use std::path::{Path, PathBuf};

use crate::cache::Cache;
use crate::config::Config;
use crate::rules::MatchKind;
use crate::write::CacheWriter;

/// What fontconfig writes into a cache directory so that backup tools skip
/// it, from the Cache Directory Tagging Specification.
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
                            # This file is a cache directory tag created by fontconfig.\n\
                            # For information about cache directory tags, see:\n\
                            #       http://www.brynosaurus.com/cachedir/\n";

/// What became of one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// The directory this describes.
    pub dir: PathBuf,
    /// Where its cache is now.
    pub cache: PathBuf,
    /// How many patterns the cache holds. One font file can contribute
    /// several.
    pub fonts: usize,
    /// The subdirectories it records, which a caller walking a tree should
    /// visit next.
    pub subdirs: Vec<PathBuf>,
    /// Whether the directory was rescanned, as opposed to an existing cache
    /// being kept.
    pub rescanned: bool,
}

/// Rebuilding the caches for font directories.
///
/// ```no_run
/// # use fontconf::{Builder, Config};
/// let config = Config::load()?;
/// let builder = Builder::new(&config);
/// for dir in config.font_dirs() {
///     for built in builder.tree(dir)? {
///         println!("{}: {} fonts", built.dir.display(), built.fonts);
///     }
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Builder<'a> {
    config: &'a Config,
    cache_dir: Option<PathBuf>,
    force: bool,
}

impl<'a> Builder<'a> {
    /// A builder using `config` for its font directories, cache directories
    /// and `target="scan"` rules.
    pub fn new(config: &'a Config) -> Self {
        Self { config, cache_dir: None, force: false }
    }

    /// Rescan even when the cache on disk looks current.
    pub fn force(&mut self, force: bool) -> &mut Self {
        self.force = force;
        self
    }

    /// Write caches here instead of into the configured cache directories.
    ///
    /// Both reading and writing move: a cache found elsewhere is ignored, so
    /// a build into a fresh directory starts from nothing. That is what makes
    /// it useful for a test.
    pub fn cache_dir(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.cache_dir = Some(path.into());
        self
    }

    /// Bring one directory's cache up to date, without descending.
    ///
    /// Returns `None` when `dir` is not a directory. Fontconfig does not
    /// treat that as an error either: a configuration naming a directory that
    /// does not exist is normal, since one config serves many machines.
    pub fn dir(&self, dir: &Path) -> io::Result<Option<Built>> {
        if !dir.is_dir() {
            return Ok(None);
        }
        let name = dir.to_string_lossy().into_owned();

        if !self.force {
            if let Some(built) = self.current(dir, &name)? {
                return Ok(Some(built));
            }
        }

        let (subdirs, files) = entries(dir)?;
        let mut fonts = Vec::new();
        for file in &files {
            // A font that cannot be read is skipped, not fatal: a directory
            // holds READMEs and licence files as well as fonts, and fontconfig
            // scans whatever is there and keeps what parses.
            let Ok(patterns) = crate::scan_file(file) else { continue };
            for mut font in patterns {
                // Configuration gets a pass over a font before it is cached.
                // Metric aliases and the rules that take a language away from
                // a font that only appears to have it are all `target="scan"`.
                self.config.substitute_kind(&mut font, MatchKind::Scan, None);
                fonts.push(font);
            }
        }

        let names: Vec<String> =
            subdirs.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let mut writer = CacheWriter::new(&name);
        let (stamp, nanoseconds) = directory_stamp(dir)?;
        writer.mtime(stamp, nanoseconds);
        for subdir in &names {
            writer.subdir(subdir);
        }
        for font in &fonts {
            writer.font(font);
        }

        let cache = self.write(&name, &writer.finish())?;
        Ok(Some(Built { dir: dir.to_path_buf(), cache, fonts: fonts.len(), subdirs, rescanned: true }))
    }

    /// Bring a whole tree up to date, depth first.
    ///
    /// Subdirectories come from each cache rather than from a second pass
    /// over the filesystem, so a directory is visited exactly as often as
    /// something records it. A cycle through symbolic links cannot loop
    /// forever: a directory already seen is not visited again.
    pub fn tree(&self, root: &Path) -> io::Result<Vec<Built>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            if !seen.insert(dir.clone()) {
                continue;
            }
            let Some(built) = self.dir(&dir)? else { continue };
            pending.extend(built.subdirs.iter().cloned());
            out.push(built);
        }
        Ok(out)
    }

    /// An existing cache for `dir`, if it is still current.
    fn current(&self, dir: &Path, name: &str) -> io::Result<Option<Built>> {
        let Some(path) = self.existing(name) else { return Ok(None) };
        let Ok(cache) = Cache::open(&path) else { return Ok(None) };
        if cache.validate().is_err() || !matches!(cache.dir(), Ok(d) if d == name) {
            return Ok(None);
        }
        let (stamp, nanoseconds) = directory_stamp(dir)?;
        // A *directory* that reports nothing at all -- a filesystem with no
        // timestamps, which is how a read-only image ships caches that never
        // expire -- makes every cache for it current. Note this is the
        // directory's stamp, not the cache's: a cache recorded with a zero
        // still has to match.
        if stamp != 0 && !matches!(cache.mtime(), Ok(t) if t == (stamp, nanoseconds)) {
            return Ok(None);
        }
        let (Ok(fonts), Ok(subdirs)) = (cache.fonts(), cache.subdirs()) else {
            return Ok(None);
        };
        let subdirs = subdirs.filter_map(|d| d.ok()).map(PathBuf::from).collect();
        Ok(Some(Built {
            dir: dir.to_path_buf(),
            cache: path,
            fonts: fonts.count(),
            subdirs,
            rescanned: false,
        }))
    }

    /// Where a cache for `name` already is.
    fn existing(&self, name: &str) -> Option<PathBuf> {
        match &self.cache_dir {
            Some(dir) => {
                let path = dir.join(Config::cache_basename(name));
                path.is_file().then_some(path)
            }
            None => self.config.cache_path(name),
        }
    }

    /// Replace the cache for `name`, atomically.
    ///
    /// The bytes go to a temporary file in the same directory and are then
    /// renamed over the old one, so a reader either sees the whole new cache
    /// or the whole old one. Writing in place would leave a window in which
    /// the header claims a length the file does not yet have, which is
    /// exactly the check fontconfig uses to reject a corrupt cache.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let dir = self.destination()?;
        let path = dir.join(Config::cache_basename(name));
        let temp = path.with_extension("NEW");
        std::fs::write(&temp, bytes)?;
        // Rename over an existing file is atomic on Unix and, since Windows
        // 10, on NTFS as well.
        match std::fs::rename(&temp, &path) {
            Ok(()) => Ok(path),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                Err(e)
            }
        }
    }

    /// The first cache directory that can be written to, creating it if it is
    /// not there.
    fn destination(&self) -> io::Result<PathBuf> {
        let candidates: Vec<&Path> = match &self.cache_dir {
            Some(dir) => vec![dir.as_path()],
            None => self.config.cache_dirs().iter().map(PathBuf::as_path).collect(),
        };
        let mut last = None;
        for dir in candidates {
            if let Err(e) = std::fs::create_dir_all(dir) {
                last = Some(e);
                continue;
            }
            // Existing but unwritable is the common case for the system cache
            // directory when running as an ordinary user, and it is why
            // fontconfig falls through to the per-user one.
            match std::fs::write(dir.join(".fontconf-probe"), b"") {
                Ok(()) => {
                    let _ = std::fs::remove_file(dir.join(".fontconf-probe"));
                    tag(dir);
                    return Ok(dir.to_path_buf());
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no cache directory is configured")
        }))
    }
}

/// Mark a cache directory so that backup and archiving tools skip it.
///
/// Best effort: fontconfig ignores a failure here too, because a cache
/// directory that cannot hold a tag file can still hold caches.
fn tag(dir: &Path) {
    let path = dir.join("CACHEDIR.TAG");
    if !path.exists() {
        let _ = std::fs::write(path, CACHEDIR_TAG);
    }
}

/// A directory's entries, split and sorted.
///
/// Fontconfig sorts the whole listing and then splits it, which comes to the
/// same thing and is why both halves are in name order. Anything comparing
/// two caches entry by entry depends on that order.
fn entries(dir: &Path) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    dirs.sort();
    files.sort();
    Ok((dirs, files))
}

/// What the cache records so a later run can tell whether the directory has
/// changed: normally its modification time.
///
/// Seconds are a signed 32-bit field in this format version and overflow in
/// 2038; fontconfig 2.18 widens it by claiming the padding word that follows.
#[cfg(not(windows))]
fn directory_stamp(dir: &Path) -> io::Result<(i32, i64)> {
    let modified = std::fs::metadata(dir)?.modified()?;
    let (seconds, nanoseconds) = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => (since.as_secs() as i32, i64::from(since.subsec_nanos())),
        // Before the epoch, which no real directory is, but the type allows.
        Err(e) => (-(e.duration().as_secs() as i32), 0),
    };
    // A reproducible build pins the clock, and a cache recording the real
    // time would differ between two builds of the same image.
    let Ok(pinned) = std::env::var("SOURCE_DATE_EPOCH") else {
        return Ok((seconds, nanoseconds));
    };
    // The nanoseconds go whenever the variable is set at all, even to
    // something unusable: it has no way to express them, so keeping the real
    // ones would defeat the whole point.
    //
    // The seconds are *clamped*, not overwritten -- a directory older than
    // the pinned time keeps its own -- and a value that will not parse is
    // ignored rather than fatal, both of which is what fontconfig does. Note
    // it has to parse wider than it is stored: a pinned time past 2038 is
    // still a legal thing to write down, it just never clamps anything.
    let clamped = pinned
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|epoch| *epoch < i64::from(seconds))
        .map_or(seconds, |epoch| epoch as i32);
    Ok((clamped, 0))
}

/// The same, for Windows, where the modification time cannot be used.
///
/// Adding a *file* to a directory does not update that directory's recorded
/// modification time -- not after three seconds, not ever, as far as a plain
/// `stat` can see. Fontconfig documents the same thing in `fcstat.c` and
/// works around it with a different Win32 call; adding a *subdirectory* does
/// update it, which is what makes the failure so easy to miss.
///
/// Trusting it here would mean a cache that never notices a new font. So the
/// field carries an Adler-32 of the directory listing instead, which is what
/// fontconfig itself puts there for filesystems whose timestamps it does not
/// trust (`FcDirChecksum`, for FAT and some network filesystems). It is the
/// same idea, not the same number: nothing else reads our value, because a
/// cache written on one machine is already tied to that machine by the
/// absolute paths inside it.
///
/// Like fontconfig's, this notices a file appearing, disappearing or being
/// renamed, and does not notice a font edited in place under the same name.
#[cfg(windows)]
fn directory_stamp(dir: &Path) -> io::Result<(i32, i64)> {
    let mut names: Vec<(bool, std::ffi::OsString)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        names.push((entry.file_type()?.is_dir(), entry.file_name()));
    }
    names.sort();

    let (mut a, mut b) = (1u32, 0u32);
    let mut eat = |byte: u8| {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    };
    for (is_dir, name) in &names {
        for byte in name.to_string_lossy().bytes() {
            eat(byte);
        }
        // A separator, so `ab` then `c` cannot hash the same as `a` then `bc`,
        // and the kind, so a file cannot be replaced by a directory unnoticed.
        eat(0);
        eat(u8::from(*is_dir));
    }
    // Never zero: a zero stamp is the format's way of saying the directory
    // has no timestamp and its cache never expires, which is not what an
    // unlucky checksum should mean.
    let sum = a | (b << 16);
    Ok((if sum == 0 { 1 } else { sum } as i32, 0))
}

#[cfg(test)]
mod tests {
    use super::{Builder, CACHEDIR_TAG};
    use crate::{Cache, Config};

    /// An empty font directory and a cache directory beside it.
    ///
    /// No font is needed: what is under test is when a cache is rebuilt and
    /// where it lands, and an empty directory still gets a cache. Scanning
    /// itself is covered against the real font set.
    fn fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("fontconf-build-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let fonts = root.join("fonts");
        let caches = root.join("caches");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::create_dir_all(&caches).unwrap();
        (fonts, caches)
    }

    fn builder<'a>(config: &'a Config, caches: &std::path::Path) -> Builder<'a> {
        let mut builder = Builder::new(config);
        builder.cache_dir(caches);
        builder
    }

    #[test]
    fn a_directory_is_scanned_and_then_left_alone() {
        let (fonts, caches) = fixture("twice");
        let config = Config::default();
        let builder = builder(&config, &caches);

        let first = builder.dir(&fonts).unwrap().expect("a directory");
        assert!(first.rescanned, "the first pass has to scan");
        assert!(first.cache.is_file());

        let second = builder.dir(&fonts).unwrap().expect("a directory");
        assert!(!second.rescanned, "the second pass should reuse the cache");
        assert_eq!(second.cache, first.cache);
        assert_eq!(second.fonts, first.fonts);
    }

    #[test]
    fn forcing_rescans_a_current_directory() {
        let (fonts, caches) = fixture("forced");
        let config = Config::default();
        let mut builder = builder(&config, &caches);
        builder.dir(&fonts).unwrap().unwrap();
        builder.force(true);
        assert!(builder.dir(&fonts).unwrap().unwrap().rescanned);
    }

    /// The cache records the directory's mtime, and only that. Touching the
    /// directory is what makes a cache stale -- nothing looks at the fonts.
    #[test]
    fn a_changed_directory_is_rescanned() {
        let (fonts, caches) = fixture("changed");
        let config = Config::default();
        let builder = builder(&config, &caches);
        builder.dir(&fonts).unwrap().unwrap();

        std::fs::write(fonts.join("README"), b"not a font").unwrap();
        assert!(
            builder.dir(&fonts).unwrap().unwrap().rescanned,
            "adding a file changes the directory mtime"
        );
    }

    /// Removing a file counts as a change too, which the mtime catches on one
    /// platform and the listing checksum on the other.
    #[test]
    fn a_removed_file_makes_the_cache_stale() {
        let (fonts, caches) = fixture("removed");
        std::fs::write(fonts.join("README"), b"not a font").unwrap();
        let config = Config::default();
        let builder = builder(&config, &caches);
        builder.dir(&fonts).unwrap().unwrap();

        std::fs::remove_file(fonts.join("README")).unwrap();
        assert!(builder.dir(&fonts).unwrap().unwrap().rescanned);
    }

    #[test]
    fn a_new_subdirectory_makes_the_cache_stale() {
        let (fonts, caches) = fixture("newsub");
        let config = Config::default();
        let builder = builder(&config, &caches);
        assert!(builder.dir(&fonts).unwrap().unwrap().subdirs.is_empty());

        std::fs::create_dir(fonts.join("child")).unwrap();
        let again = builder.dir(&fonts).unwrap().unwrap();
        assert!(again.rescanned);
        assert_eq!(again.subdirs, [fonts.join("child")]);
    }

    /// A cache describing some other directory must not be mistaken for this
    /// one's. Two directories can only collide here through an MD5 collision
    /// or a copied cache file, but the check costs nothing.
    #[test]
    fn a_cache_for_another_directory_is_ignored() {
        let (fonts, caches) = fixture("mismatched");
        let config = Config::default();
        let builder = builder(&config, &caches);
        let built = builder.dir(&fonts).unwrap().unwrap();

        let mut writer = crate::CacheWriter::new("/somewhere/else");
        writer.mtime(0, 0);
        std::fs::write(&built.cache, writer.finish()).unwrap();
        assert!(builder.dir(&fonts).unwrap().unwrap().rescanned);
    }

    #[test]
    fn the_cache_names_the_directory_it_describes() {
        let (fonts, caches) = fixture("named");
        let config = Config::default();
        let built = builder(&config, &caches).dir(&fonts).unwrap().unwrap();
        let cache = Cache::open(&built.cache).unwrap();
        cache.validate().unwrap();
        assert_eq!(cache.dir().unwrap(), fonts.to_string_lossy());
        assert_eq!(built.cache.file_name().unwrap().to_string_lossy(),
                   Config::cache_basename(&fonts.to_string_lossy()));
    }

    #[test]
    fn subdirectories_are_recorded_and_walked() {
        let (fonts, caches) = fixture("tree");
        std::fs::create_dir_all(fonts.join("child/grandchild")).unwrap();
        let config = Config::default();
        let built = builder(&config, &caches).tree(&fonts).unwrap();

        let dirs: std::collections::HashSet<_> = built.iter().map(|b| b.dir.clone()).collect();
        assert!(dirs.contains(&fonts));
        assert!(dirs.contains(&fonts.join("child")));
        assert!(dirs.contains(&fonts.join("child/grandchild")));
        assert_eq!(dirs.len(), 3);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        let (fonts, caches) = fixture("missing");
        let config = Config::default();
        assert_eq!(builder(&config, &caches).dir(&fonts.join("nope")).unwrap(), None);
    }

    /// Zero means "this directory has no timestamp, so its cache never
    /// expires". A checksum must not be able to say that by accident.
    #[cfg(windows)]
    #[test]
    fn the_listing_checksum_is_never_zero() {
        let (fonts, _) = fixture("nonzero");
        assert_ne!(super::directory_stamp(&fonts).unwrap().0, 0, "an empty directory");
        std::fs::write(fonts.join("a"), b"").unwrap();
        assert_ne!(super::directory_stamp(&fonts).unwrap().0, 0);
    }

    #[test]
    fn the_cache_directory_gets_a_tag_file() {
        let (fonts, caches) = fixture("tagged");
        let config = Config::default();
        builder(&config, &caches).dir(&fonts).unwrap();
        let tag = std::fs::read_to_string(caches.join("CACHEDIR.TAG")).unwrap();
        assert_eq!(tag, CACHEDIR_TAG);
        assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55\n"));
    }

    /// Nothing may be left behind: a `.NEW` file surviving a write would be
    /// picked up as a cache by nothing, but would grow without bound.
    #[test]
    fn no_temporary_file_is_left_behind() {
        let (fonts, caches) = fixture("temp");
        let config = Config::default();
        builder(&config, &caches).dir(&fonts).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&caches)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".NEW"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
