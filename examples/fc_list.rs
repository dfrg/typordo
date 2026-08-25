//! List the fonts this system is configured to have, for comparison with `fc-list`.
//!
//! ```text
//! cargo run --example fc_list
//! cargo run --example fc_list -- --format file
//! cargo run --example fc_list -- --config /etc/fonts/fonts.conf --dirs
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use typordo::{CachePolicy, Config, Object, PatternRef, ValueRef};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut format = "full".to_string();
    let mut config_path: Option<PathBuf> = None;
    let (mut stats, mut show_dirs) = (false, false);

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => format = args.next().unwrap_or_default(),
            "--config" => config_path = args.next().map(PathBuf::from),
            "--dirs" => show_dirs = true,
            "--stats" => stats = true,
            other => return Err(format!("unknown argument {other}").into()),
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

    if show_dirs {
        println!("config files ({}):", config.files().len());
        for file in config.files() {
            println!("  {}", file.display());
        }
        println!("font dirs:");
        for dir in config.font_dirs() {
            println!("  {}", dir.display());
        }
        println!("cache dirs:");
        for dir in config.cache_dirs() {
            println!("  {}", dir.display());
        }
        return Ok(());
    }

    let mut lines = BTreeSet::new();
    let (mut caches, mut patterns) = (0usize, 0usize);

    for (dir, cache) in config.caches(CachePolicy::read_only()) {
        cache.validate().map_err(|e| format!("{dir}: {e}"))?;
        caches += 1;
        for font in cache.fonts()? {
            // <selectfont> decides what is listed at all.
            if !config.accepts(&font) {
                continue;
            }
            patterns += 1;
            let file = font.string(Object::File).unwrap_or("<no file>");
            match format.as_str() {
                "file" => {
                    lines.insert(file.to_string());
                }
                "family" => {
                    if let Some(family) = font.string(Object::Family) {
                        lines.insert(family.to_string());
                    }
                }
                _ => {
                    // `fc-list` prints every family and every style a pattern
                    // holds, comma separated, in the order they are stored.
                    let families = join(font, Object::Family);
                    let styles = join(font, Object::Style);
                    lines.insert(format!("{file}: {families}:style={styles}"));
                }
            }
        }
    }

    for line in &lines {
        println!("{line}");
    }

    if stats {
        eprintln!(
            "{} config files, {} font dirs, {} caches, {} patterns, {} lines, selectfont: {}",
            config.files().len(),
            config.font_dirs().len(),
            caches,
            patterns,
            lines.len(),
            config.has_selectors()
        );
    }
    Ok(())
}

/// Every string value of `object`, comma separated, the way `fc-list` prints them.
fn join(font: PatternRef<'_>, object: Object) -> String {
    let mut out = String::new();
    let Some(element) = font.get(object) else {
        return out;
    };
    for value in element.values() {
        if let ValueRef::String(s) = value {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(s);
        }
    }
    out
}
