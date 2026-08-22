//! Print the order the cache walk visits directories in.
use std::path::PathBuf;
use fontconf::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--config" {
            path = args.next().map(PathBuf::from);
        }
    }
    let config = match &path {
        Some(p) => Config::load_from(p)?,
        None => Config::load()?,
    };
    for (dir, _cache) in config.caches() {
        println!("{dir}");
    }
    Ok(())
}
