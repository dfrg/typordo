//! Print each font's coverage as hex ranges, to compare with `fc-query`.
//!
//! ```text
//! cargo run --example charset -- /usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf
//! ```

use fontconf::{Config, Object, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let want: Vec<String> = std::env::args().skip(1).collect();
    let config = Config::load()?;
    for (_dir, cache) in config.caches() {
        for font in cache.fonts()? {
            let Some(file) = font.string(Object::File) else { continue };
            if !want.is_empty() && !want.iter().any(|w| w == file) {
                continue;
            }
            let Some(Value::CharSet(charset)) = font.value(Object::Charset) else {
                continue;
            };
            charset.validate()?;
            println!("{charset}");
            if want.is_empty() {
                eprintln!("  {file}: {} chars", charset.len());
            }
        }
    }
    Ok(())
}
