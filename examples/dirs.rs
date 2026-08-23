//! Print what the configuration says about directories and their caches.
//!
//! ```text
//! cargo run --example dirs -- --config /etc/fonts/fonts.conf
//! cargo run --example dirs -- --cache-name /usr/share/fonts
//! cargo run --example dirs -- --cache-path /usr/share/fonts
//! ```

use std::path::PathBuf;

use fontconf::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let (mut name_of, mut path_of) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            // The name a cache for this directory would have, which is what
            // fontconfig prints under FC_DEBUG=16.
            "--cache-name" => name_of = args.next(),
            // Where that cache actually is, if it is anywhere.
            "--cache-path" => path_of = args.next(),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let config = match &path {
        Some(p) => Config::load_from(p)?,
        None => Config::load()?,
    };

    if let Some(dir) = &name_of {
        println!("{}", config.cache_basename(dir));
        return Ok(());
    }
    if let Some(dir) = &path_of {
        if let Some(found) = config.cache_path(dir) {
            println!("{}", found.display());
        }
        return Ok(());
    }

    for (dir, _cache) in config.caches() {
        println!("{dir}");
    }
    Ok(())
}
