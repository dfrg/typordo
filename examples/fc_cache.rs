//! Write cache files, for comparison with `fc-cache`.
//!
//! Two modes, because they test different halves. `--rewrite` reads the
//! caches this system already has and writes them out again, which exercises
//! the writer alone: whatever comes back has to match what went in, and
//! fontconfig itself has to accept the result. Scanning builds a cache from
//! the font files, which exercises the scanner as well.
//!
//! ```text
//! cargo run --example fc_cache -- --out /tmp/cache --rewrite
//! cargo run --example fc_cache -- --out /tmp/cache /usr/share/fonts
//! ```

use std::path::{Path, PathBuf};

use fontconf::{Cache, CacheWriter, Config, MatchKind, Query};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut rewrite = false;
    let mut dirs: Vec<PathBuf> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().map(PathBuf::from),
            "--config" => config_path = args.next().map(PathBuf::from),
            "--rewrite" => rewrite = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument {other}").into())
            }
            other => dirs.push(PathBuf::from(other)),
        }
    }
    let out = out.ok_or("--out <directory> is required")?;
    std::fs::create_dir_all(&out)?;

    let config = match &config_path {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };

    let (mut written, mut fonts) = (0usize, 0usize);
    if rewrite {
        for (dir, cache) in config.caches() {
            let (n, bytes) = rewrite_cache(&cache)?;
            written += 1;
            fonts += n;
            std::fs::write(out.join(Config::cache_basename(&dir)), bytes)?;
        }
    } else {
        for dir in &dirs {
            let mut pending = vec![dir.clone()];
            while let Some(dir) = pending.pop() {
                if !dir.is_dir() {
                    continue;
                }
                let (subdirs, n, bytes) = scan_dir(&dir, &config)?;
                pending.extend(subdirs);
                written += 1;
                fonts += n;
                let name = Config::cache_basename(&dir.to_string_lossy());
                std::fs::write(out.join(name), bytes)?;
            }
        }
    }
    println!("wrote {written} caches, {fonts} fonts, into {}", out.display());
    Ok(())
}

/// Write a cache back out, keeping the directory it describes.
fn rewrite_cache(cache: &Cache) -> Result<(usize, Vec<u8>), Box<dyn std::error::Error>> {
    cache.validate()?;
    let dir = cache.dir()?.to_string();
    let subdirs: Vec<String> =
        cache.subdirs()?.collect::<Result<Vec<_>, _>>()?.iter().map(|s| s.to_string()).collect();
    let fonts: Vec<Query> = cache.fonts()?.map(|p| Query::from_pattern(&p)).collect();
    let (seconds, nanoseconds) = cache.mtime()?;

    let mut writer = CacheWriter::new(&dir);
    writer.mtime(seconds, nanoseconds);
    for subdir in &subdirs {
        writer.subdir(subdir);
    }
    for font in &fonts {
        writer.font(font);
    }
    Ok((fonts.len(), writer.finish()))
}

/// What scanning one directory produced: its subdirectories, how many fonts
/// it held, and the cache bytes.
type Scanned = (Vec<PathBuf>, usize, Vec<u8>);

/// Scan one directory, without descending: fontconfig gives every directory
/// its own cache and records the children by name.
#[cfg(feature = "scan")]
fn scan_dir(
    dir: &Path,
    config: &Config,
) -> Result<Scanned, Box<dyn std::error::Error>> {
    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            subdirs.push(path);
        } else {
            files.push(path);
        }
    }
    // Fontconfig lists a directory in name order, and the cache keeps that
    // order, so anything comparing two caches entry by entry depends on it.
    subdirs.sort();
    files.sort();

    // A scanned font is not what goes in the cache: configuration gets a
    // pass over it first. That is how DejaVu Math TeX Gyre ends up filed
    // under DejaVu Serif as well, and how Book gains its Regular alias --
    // metric aliases and the generic-family rules are all `target="scan"`.
    let mut fonts = Vec::new();
    for file in &files {
        let Ok(patterns) = fontconf::scan_file(file) else { continue };
        for mut font in patterns {
            config.substitute_kind(&mut font, MatchKind::Scan, None);
            fonts.push(font);
        }
    }

    let path = dir.to_string_lossy();
    let names: Vec<String> = subdirs.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let mut writer = CacheWriter::new(&path);
    let mtime = std::fs::metadata(dir)?.modified()?;
    let since = mtime.duration_since(std::time::UNIX_EPOCH)?;
    writer.mtime(since.as_secs() as i32, i64::from(since.subsec_nanos()));
    for name in &names {
        writer.subdir(name);
    }
    for font in &fonts {
        writer.font(font);
    }
    Ok((subdirs, fonts.len(), writer.finish()))
}

#[cfg(not(feature = "scan"))]
fn scan_dir(
    _dir: &Path,
    _config: &Config,
) -> Result<Scanned, Box<dyn std::error::Error>> {
    Err("scanning needs the `scan` feature; --rewrite works without it".into())
}
