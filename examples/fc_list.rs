//! Dump every font in a directory of cache files, for comparison with `fc-list`.
//!
//! ```text
//! cargo run --example fc_list -- ~/.cache/fontconfig
//! cargo run --example fc_list -- ~/.cache/fontconfig --format file
//! ```
//!
//! This deliberately reads whatever caches are in the directory rather than
//! consulting a configuration, so it is a check on the cache reader alone.
//! Which directories a system actually considers is a configuration question,
//! and answering it is the next slice of work, not this one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use fontconf::{Cache, Object, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| {
        eprintln!("usage: fc_list <cache-dir> [--format file|family|full] [--stats]");
        std::process::exit(2)
    }));

    let mut format = "full".to_string();
    let mut stats = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => format = args.next().unwrap_or_default(),
            "--stats" => stats = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }

    let mut caches = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.starts_with("cache-")) {
            caches.push(path);
        }
    }
    caches.sort();

    let mut lines = BTreeSet::new();
    let (mut patterns, mut bytes) = (0usize, 0usize);

    for path in &caches {
        let cache = Cache::open(path)?;
        // Strict walk: a cache that does not hold up should fail loudly here
        // rather than quietly contributing fewer fonts than it has.
        cache.validate().map_err(|e| format!("{}: {e}", path.display()))?;
        bytes += cache.as_bytes().len();

        for font in cache.fonts()? {
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
            "{} caches, {} bytes, {} patterns, {} distinct lines",
            caches.len(),
            bytes,
            patterns,
            lines.len()
        );
    }
    Ok(())
}

/// Every string value of `object`, comma separated, the way `fc-list` prints them.
fn join(font: fontconf::Pattern<'_>, object: Object) -> String {
    let mut out = String::new();
    let Some(element) = font.get(object) else {
        return out;
    };
    for value in element.values() {
        if let Value::String(s) = value {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(s);
        }
    }
    out
}
