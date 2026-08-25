//! Build font caches, for comparison with `fc-cache`.
//!
//! With no directories named it does what `fc-cache` does with none: every
//! font directory the configuration knows about, and everything beneath them.
//!
//! `--rewrite` is the other half, and exists for testing: it reads the caches
//! this system already has and writes them back out, exercising the writer
//! without the scanner in the way.
//!
//! ```text
//! cargo run --features scan --example fc_cache -- -v
//! cargo run --features scan --example fc_cache -- --out /tmp/cache -f /usr/share/fonts
//! cargo run --features scan --example fc_cache -- --out /tmp/cache --rewrite
//! ```

use std::path::{Path, PathBuf};

use typordo::{Builder, Cache, CachePolicy, CacheWriter, Config, Pattern};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let (mut rewrite, mut force, mut verbose) = (false, false, false);
    let mut dirs: Vec<PathBuf> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().map(PathBuf::from),
            "--config" => config_path = args.next().map(PathBuf::from),
            "--rewrite" => rewrite = true,
            "-f" | "--force" => force = true,
            "-v" | "--verbose" => verbose = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown argument {other}").into())
            }
            other => dirs.push(PathBuf::from(other)),
        }
    }

    let config = match &config_path {
        // fc-list and friends do not stop when a configuration will not
        // load: `FcInitLoadOwnConfig` runs on the built-in fallback
        // instead. Doing the same is what makes a comparison against
        // them meaningful when the config under test is a broken one.
        Some(path) => match Config::load_from(path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("cannot load {}: {e}", path.display());
                Config::fallback(None)?
            }
        },
        None => Config::load()?,
    };

    if rewrite {
        let out = out.ok_or("--rewrite needs --out <directory>")?;
        std::fs::create_dir_all(&out)?;
        let (mut written, mut fonts) = (0, 0);
        for (dir, cache) in config.caches(CachePolicy::read_only()) {
            let (n, bytes) = rewrite_cache(&cache)?;
            std::fs::write(out.join(config.cache_basename(&dir)), bytes)?;
            written += 1;
            fonts += n;
        }
        println!("rewrote {written} caches, {fonts} fonts, into {}", out.display());
        return Ok(());
    }

    let mut builder = Builder::new(&config);
    builder.force(force);
    if let Some(out) = &out {
        builder.cache_dir(out);
    }

    let roots: Vec<PathBuf> =
        if dirs.is_empty() { config.font_dirs().map(Path::to_path_buf).collect() } else { dirs };

    let (mut scanned, mut kept, mut fonts) = (0, 0, 0);
    for root in &roots {
        for built in builder.tree(root)? {
            if built.rescanned {
                scanned += 1;
            } else {
                kept += 1;
            }
            fonts += built.fonts;
            if verbose {
                let what = if built.rescanned { "cached" } else { "current" };
                println!(
                    "{}: {what}, {} fonts, {} dirs",
                    built.dir.display(),
                    built.fonts,
                    built.subdirs.len()
                );
            }
        }
    }
    println!("{scanned} directories rescanned, {kept} already current, {fonts} fonts");
    Ok(())
}

/// Write a cache back out, keeping the directory it describes.
fn rewrite_cache(cache: &Cache) -> Result<(usize, Vec<u8>), Box<dyn std::error::Error>> {
    cache.validate()?;
    let dir = cache.dir()?.to_string();
    let subdirs: Vec<String> =
        cache.subdirs()?.collect::<Result<Vec<_>, _>>()?.iter().map(|s| s.to_string()).collect();
    let fonts: Vec<Pattern> = cache.fonts()?.map(|p| Pattern::from_pattern(&p)).collect();
    let (stamp, nanoseconds) = cache.mtime()?;

    let mut writer = CacheWriter::new(&dir);
    writer.mtime(stamp, nanoseconds);
    for subdir in &subdirs {
        writer.subdir(subdir);
    }
    for font in &fonts {
        writer.font(font);
    }
    Ok((fonts.len(), writer.finish()))
}
