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
use crate::object::Object;
use crate::rules::MatchKind;
use crate::stamp::directory_stamp;
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
/// # use typordo::{Builder, Config};
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
        // `dir` is named as the target sees it; `host` is where to read it.
        // The two differ only under a sysroot, and everything recorded in the
        // cache uses the target name -- fontconfig strips the sysroot back
        // off `FC_FILE` for the same reason, so that a cache built for an
        // image describes the image rather than the machine that built it.
        let host = self.config.host_path(dir);
        if !host.is_dir() {
            return Ok(None);
        }
        let name = dir.to_string_lossy().into_owned();

        if !self.force {
            if let Some(built) = self.current(dir, &name)? {
                return Ok(Some(built));
            }
        }

        let (subdirs, files) = entries(&host)?;
        let mut fonts = Vec::new();
        for file in &files {
            // A font that cannot be read is skipped, not fatal: a directory
            // holds READMEs and licence files as well as fonts, and fontconfig
            // scans whatever is there and keeps what parses.
            let Ok(patterns) = crate::scan_file(file) else { continue };
            for mut font in patterns {
                // Recorded as the target names it, not as it was reached.
                if self.config.sysroot().is_some() {
                    let target = self.config.target_path(file);
                    font.remove(Object::File);
                    font.add(Object::File, target.to_string_lossy().as_ref());
                }
                // Configuration gets a pass over a font before it is cached.
                // Metric aliases and the rules that take a language away from
                // a font that only appears to have it are all `target="scan"`.
                self.config.substitute_kind(&mut font, MatchKind::Scan, None);
                fonts.push(font);
            }
        }

        let names: Vec<String> = subdirs
            .iter()
            .map(|p| self.config.target_path(p).to_string_lossy().into_owned())
            .collect();
        let mut writer = CacheWriter::new(&name);
        let (stamp, nanoseconds) = directory_stamp(&host)?;
        writer.mtime(stamp, nanoseconds);
        for subdir in &names {
            writer.subdir(subdir);
        }
        for font in &fonts {
            writer.font(font);
        }

        let cache = self.write(&name, &writer.finish())?;
        Ok(Some(Built {
            dir: dir.to_path_buf(),
            cache,
            fonts: fonts.len(),
            subdirs,
            rescanned: true,
        }))
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
                let path = dir.join(self.config.cache_basename(name));
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
        let path = dir.join(self.config.cache_basename(name));
        // The cache lands under the sysroot, where the target will find it.
        let on_disk = self.config.host_path(&path);
        if let Some(parent) = on_disk.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The lock is what makes the fixed `.NEW` name safe. Two processes
        // rebuilding the same directory would otherwise write the same
        // temporary file, and each could rename the other's half of it into
        // place. `fc-cache` and a desktop session starting at once is not a
        // hypothetical; it is the usual way a machine boots.
        let lock = Lock::take(&on_disk)?;
        lock.clear_stale_new(&on_disk);
        // `with_suffix`, not `with_extension`: the latter would replace the
        // `.cache-9` and write `<hash>-le64.NEW`, which is neither the name
        // fontconfig uses nor one the lock beside it protects.
        let temp = with_suffix(&on_disk, ".NEW");
        std::fs::write(&temp, bytes)?;
        // Rename over an existing file is atomic on Unix and, since Windows
        // 10, on NTFS as well.
        match std::fs::rename(&temp, &on_disk) {
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
            match std::fs::write(dir.join(".typordo-probe"), b"") {
                Ok(()) => {
                    let _ = std::fs::remove_file(dir.join(".typordo-probe"));
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

/// How long a lock may sit before it is assumed to belong to a dead process.
///
/// `FcAtomicLock`'s ten minutes, and its assumption with it: that machines
/// sharing a filesystem keep their clocks reasonably close. A shorter timeout
/// would steal the lock from a slow but living rebuild; a longer one leaves a
/// crashed process blocking every other for that much longer.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// An exclusive claim on one cache file, held while it is replaced.
///
/// `FcAtomicLock`: a `<cache>.LCK` beside the cache, created by hard-linking
/// a temporary file that names the owning process. The hard link is the
/// atomic part -- `link` fails if the destination exists, and it fails for
/// every process but one. Filesystems that refuse hard links fall back to
/// `mkdir`, which is atomic for the same reason.
///
/// Released on drop, including on the error paths, which is the difference
/// between a failed rebuild and a directory nothing can rebuild for the next
/// ten minutes.
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn take(cache: &Path) -> io::Result<Self> {
        let path = with_suffix(cache, ".LCK");
        match Self::claim(cache, &path) {
            Ok(()) => Ok(Self { path }),
            Err(e) => {
                // Someone holds it. If it is old enough to have been
                // abandoned, take it over; fontconfig does the same, and for
                // the same reason -- a crashed `fc-cache` should not stop the
                // machine caching fonts.
                if !Self::is_stale(&path) {
                    return Err(e);
                }
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_dir(&path);
                Self::claim(cache, &path).map(|()| Self { path })
            }
        }
    }

    fn claim(cache: &Path, lock: &Path) -> io::Result<()> {
        // The temporary carries our process id, as fontconfig's does: it is
        // never read back, but it is what makes a leftover lock diagnosable.
        let temp = with_suffix(cache, &format!(".TMP-{}", std::process::id()));
        std::fs::write(
            &temp,
            format!(
                "{}
",
                std::process::id()
            ),
        )?;
        let linked = std::fs::hard_link(&temp, lock);
        let _ = std::fs::remove_file(&temp);
        match linked {
            Ok(()) => Ok(()),
            // A filesystem that will not hard-link -- some network mounts,
            // and FAT -- still creates directories atomically.
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => match std::fs::create_dir(lock) {
                Ok(()) => Ok(()),
                Err(_) => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    /// Remove a `.NEW` left behind by a writer that did not finish.
    ///
    /// `FcAtomicLock` unlinks it on the way out, and it is safe here for the
    /// same reason: holding the lock means nobody else is writing that name.
    fn clear_stale_new(&self, cache: &Path) {
        let _ = std::fs::remove_file(with_suffix(cache, ".NEW"));
    }

    fn is_stale(lock: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(lock) else { return false };
        let Ok(modified) = meta.modified() else { return false };
        modified.elapsed().is_ok_and(|age| age > LOCK_TIMEOUT)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Whichever way it was taken.
        if std::fs::remove_file(&self.path).is_err() {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

/// `path` with `suffix` appended to its file name.
///
/// Not `with_extension`, which would replace the `.cache-9` a cache file ends
/// with rather than adding to it.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
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
    /// Assert that a mutation actually moved the directory's stamp.
    ///
    /// The staleness tests all say "change the directory, expect a rescan",
    /// and that only follows when the change is one the filesystem can
    /// express. `freshness` compares the recorded stamp against the current
    /// one for exact equality, so on a filesystem with nanosecond timestamps
    /// any mutation moves it -- but a coarse one can put the write and the
    /// cache in the same tick, and then fontconfig would not rescan either.
    ///
    /// Without this the failure reads as "the cache was not stale", which
    /// sends the next person into the cache code. It is not a cache problem.
    fn stamp_moved(dir: &std::path::Path, before: (i32, i64)) {
        let after = crate::stamp::directory_stamp(dir).expect("the directory is still there");
        assert_ne!(
            after, before,
            "this filesystem gave the same stamp before and after the change,              so nothing could detect it -- the timestamp granularity is too              coarse for this test, not the staleness check"
        );
    }

    fn fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("typordo-build-{name}"));
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

        let before = crate::stamp::directory_stamp(&fonts).unwrap();
        std::fs::write(fonts.join("README"), b"not a font").unwrap();
        stamp_moved(&fonts, before);
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

        let before = crate::stamp::directory_stamp(&fonts).unwrap();
        std::fs::remove_file(fonts.join("README")).unwrap();
        stamp_moved(&fonts, before);
        assert!(builder.dir(&fonts).unwrap().unwrap().rescanned);
    }

    #[test]
    fn a_new_subdirectory_makes_the_cache_stale() {
        let (fonts, caches) = fixture("newsub");
        let config = Config::default();
        let builder = builder(&config, &caches);
        assert!(builder.dir(&fonts).unwrap().unwrap().subdirs.is_empty());

        let before = crate::stamp::directory_stamp(&fonts).unwrap();
        std::fs::create_dir(fonts.join("child")).unwrap();
        stamp_moved(&fonts, before);
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
        assert_eq!(
            built.cache.file_name().unwrap().to_string_lossy(),
            config.cache_basename(&fonts.to_string_lossy())
        );
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

#[cfg(test)]
mod lock_tests {
    use super::{with_suffix, Lock};
    use std::path::Path;

    /// A cache file ends `.cache-9`, so `with_extension` replaces the version
    /// rather than adding to the name. For the lock that would produce a file
    /// which excludes nobody; for the temporary it produced a name fontconfig
    /// does not use. Both were written with `with_extension` first, which is
    /// why this is spelled out.
    #[test]
    fn a_suffix_is_added_not_substituted() {
        let path = Path::new("/tmp/abc-le64.cache-9");
        assert_eq!(with_suffix(path, ".LCK"), Path::new("/tmp/abc-le64.cache-9.LCK"));
        assert_eq!(with_suffix(path, ".NEW"), Path::new("/tmp/abc-le64.cache-9.NEW"));
        assert_ne!(with_suffix(path, ".LCK"), path.with_extension("LCK"));
        assert_ne!(with_suffix(path, ".NEW"), path.with_extension("NEW"));
    }

    #[test]
    fn a_lock_excludes_a_second_taker_and_releases_on_drop() {
        let dir = std::env::temp_dir().join("typordo-lock-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("abc-le64.cache-9");

        let held = Lock::take(&cache).expect("the first taker gets it");
        assert!(with_suffix(&cache, ".LCK").exists(), "the lock file should be there");
        assert!(Lock::take(&cache).is_err(), "a second taker must be refused");
        drop(held);
        assert!(!with_suffix(&cache, ".LCK").exists(), "dropping releases it");

        // And it can be taken again afterwards.
        let again = Lock::take(&cache).expect("released means available");
        drop(again);

        // No temporary is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lock left by a process that died must not block rebuilding for ever.
    /// The age is read from the file's own timestamp, so the test sets one.
    #[test]
    fn a_lock_old_enough_to_be_abandoned_is_taken_over() {
        let dir = std::env::temp_dir().join("typordo-stale-lock-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("abc-le64.cache-9");
        let lock = with_suffix(&cache, ".LCK");

        std::fs::write(&lock, b"99999\n").unwrap();
        assert!(Lock::take(&cache).is_err(), "a fresh lock is respected");

        // Back-date it past the timeout.
        let old =
            std::time::SystemTime::now() - super::LOCK_TIMEOUT - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
        let taken = Lock::take(&cache).expect("a stale lock is taken over");
        drop(taken);
        assert!(!lock.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
