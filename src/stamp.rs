//! Whether a directory has changed since its cache was written.
//!
//! This is a question about *reading*: deciding whether a cache still
//! describes its directory has to be answerable without the machinery for
//! building one, so it lives here rather than beside the builder that also
//! uses it.
//!
//! The cache carries one number for the directory it came from. Normally that
//! is the modification time; where a modification time cannot be trusted it
//! is a checksum of the listing instead, which is the substitution fontconfig
//! makes for the same reason.

use std::io;
use std::path::Path;

use crate::cache::Cache;

/// Whether `cache` still describes the directory it was written for.
///
/// The comparison fontconfig makes in `FcDirCacheValidateHelper`, and the
/// same one the builder makes before deciding a cache needs no work.
/// What a directory has to say about the cache written for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// The cache still describes the directory.
    Current,
    /// The directory has changed since the cache was written.
    Stale,
    /// The directory cannot be read at all.
    Gone,
}

pub(crate) fn freshness(dir: &str, cache: &Cache) -> Freshness {
    let Ok((stamp, nanoseconds)) = directory_stamp(Path::new(dir)) else {
        // Removed, unmounted, or no longer permitted. Fontconfig gives up on
        // the directory here -- `FcDirCacheProcess` fails on the stat, and so
        // does the rescan it falls back to -- and the reason is better than
        // the symmetry: the font files are gone with the directory, so a
        // cache describing them answers with paths that no longer open.
        //
        // This used to report `Current`, on the argument that the cache was
        // the only description left. It is a description of nothing.
        return Freshness::Gone;
    };
    // A *directory* that reports nothing at all -- a filesystem with no
    // timestamps, which is how a read-only image ships caches that never
    // expire -- makes every cache for it current. Note this is the
    // directory's stamp, not the cache's: a cache recorded with a zero still
    // has to match.
    if stamp == 0 {
        return Freshness::Current;
    }
    match cache.mtime() {
        Ok(recorded) if recorded == (stamp, nanoseconds) => Freshness::Current,
        _ => Freshness::Stale,
    }
}

/// What the cache records so a later run can tell whether the directory has
/// changed: normally its modification time.
///
/// Seconds are a signed 32-bit field in this format version and overflow in
/// 2038; fontconfig 2.18 widens it by claiming the padding word that follows.
#[cfg(not(windows))]
pub(crate) fn directory_stamp(dir: &Path) -> io::Result<(i32, i64)> {
    if mtime_is_broken(dir) {
        return Ok((listing_checksum(dir)?, 0));
    }
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

/// Whether this directory sits on a filesystem whose timestamps lie.
///
/// FAT records a directory time too coarsely for fontconfig to trust, so it
/// asks `statfs` and falls back to hashing the listing. Answering the
/// question needs a libc call, which is why it is behind a feature; without
/// it the timestamp is trusted everywhere, which is right for every
/// filesystem except the ones this exists for.
#[cfg(all(not(windows), feature = "statfs"))]
pub(crate) fn mtime_is_broken(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    /// `MSDOS_SUPER_MAGIC`, the one type fontconfig singles out.
    #[cfg(target_os = "linux")]
    const MSDOS: i64 = 0x4d44;

    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `statfs` fills the struct it is given and reads only the path,
    // which is NUL-terminated by CString. A failure leaves the struct
    // untouched, which is why the return value is checked before it is read.
    #[allow(unsafe_code)]
    unsafe {
        let mut buf = std::mem::zeroed::<libc::statfs>();
        if libc::statfs(path.as_ptr(), &mut buf) != 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            // `f_type` is `__fsword_t`: 64-bit on some targets, 32-bit and
            // unsigned on others, so the cast is what makes the comparison
            // compile everywhere rather than a conversion that does work.
            buf.f_type as i64 == MSDOS
        }
        #[cfg(not(target_os = "linux"))]
        {
            let name = std::ffi::CStr::from_ptr(buf.f_fstypename.as_ptr());
            matches!(name.to_bytes(), b"msdosfs" | b"pcfs")
        }
    }
}

/// Without the `statfs` feature, every Unix filesystem is trusted.
#[cfg(all(not(windows), not(feature = "statfs")))]
pub(crate) fn mtime_is_broken(_dir: &Path) -> bool {
    false
}

/// An Adler-32 of the directory listing, for when the timestamp cannot be
/// used. See the Windows [`directory_stamp`] for why this is the fallback
/// fontconfig also reaches for.
#[cfg(not(windows))]
fn listing_checksum(dir: &Path) -> io::Result<i32> {
    listing_adler32(dir)
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
pub(crate) fn directory_stamp(dir: &Path) -> io::Result<(i32, i64)> {
    Ok((listing_adler32(dir)?, 0))
}

/// An Adler-32 over the sorted directory listing.
///
/// Notices a file appearing, disappearing or being renamed; does not notice
/// one edited in place under the same name. That is exactly what fontconfig
/// records in the same field for filesystems whose timestamps it does not
/// trust, and it has the same blind spot.
fn listing_adler32(dir: &Path) -> io::Result<i32> {
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
    Ok(if sum == 0 { 1 } else { sum as i32 })
}
