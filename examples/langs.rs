//! Print each font's languages, to compare with `fc-list --format='%{lang}'`.
use fontconf::{CachePolicy, Config, Object, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let want: Vec<String> = std::env::args().skip(1).collect();
    let config = Config::load()?;
    for (_dir, cache) in config.caches(CachePolicy::read_only()) {
        for font in cache.fonts()? {
            let Some(file) = font.string(Object::File) else { continue };
            if !want.is_empty() && !want.iter().any(|w| w == file) {
                continue;
            }
            match font.value(Object::Lang) {
                Some(Value::LangSet(langset)) => {
                    langset.validate()?;
                    if !langset.is_consistent() {
                        eprintln!("{file}: bitmap wider than our language table");
                    }
                    println!("{file}	{langset}");
                }
                _ => println!("{file}	"),
            }
        }
    }
    Ok(())
}
