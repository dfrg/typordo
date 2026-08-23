//! Build a fallback chain for a string, the way a text layout engine would.
//!
//! One font rarely covers a whole run of text. A layout engine asks for an
//! ordered list rather than a single answer, then walks it: the first font
//! takes what it can, the next takes what is left, and so on.
//!
//! ```text
//! cargo run --example fallback_chain -- "Hello Ελλάς 日本語 🎉"
//! ```

use std::error::Error;

use fontconf::{sort, Config, Object, Pattern, Query, Value};

fn main() -> Result<(), Box<dyn Error>> {
    let text: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        eprintln!("usage: fallback_chain <text>");
        std::process::exit(2);
    }

    let config = Config::load()?;
    let caches: Vec<_> = config.caches().collect();
    let fonts: Vec<Pattern<'_>> = caches
        .iter()
        .filter_map(|(_, cache)| cache.fonts().ok())
        .flatten()
        .filter(|font| config.accepts(font))
        .collect();

    let mut query = Query::new();
    query.add(Object::Family, "sans-serif");
    config.substitute(&mut query);
    query.default_substitute();

    // `sort` returns every font ranked best-first. Trimming drops the ones
    // that add no coverage the fonts before them did not already have, which
    // is what makes the result a *chain* rather than a ranking -- without it
    // the list is thousands of fonts long and mostly redundant.
    let ranked = sort(&query, fonts.iter().cloned(), true);
    println!("{} fonts in the trimmed chain\n", ranked.len());

    // Walk the text once, charging each character to the first font in the
    // chain that covers it. This is the loop a layout engine runs, and the
    // reason the order matters: the same character is often in many fonts,
    // and the chain decides which one draws it.
    //
    // Each character is charged once, so what is printed under a font is the
    // set of distinct characters it claimed -- "Hello" shows as "Helo".
    let mut seen = std::collections::HashSet::new();
    let mut remaining: Vec<char> =
        text.chars().filter(|c| !c.is_whitespace() && seen.insert(*c)).collect();

    for (font, _) in &ranked {
        if remaining.is_empty() {
            break;
        }
        let Some(Value::CharSet(coverage)) = font.value(Object::Charset) else { continue };

        let (covered, left): (Vec<char>, Vec<char>) =
            remaining.iter().partition(|c| coverage.contains(**c));
        if covered.is_empty() {
            continue;
        }
        remaining = left;

        let family = font.string(Object::Family).unwrap_or("?");
        let style = font.string(Object::Style).unwrap_or("");
        println!("{family} {style}");
        println!("    {}", covered.iter().collect::<String>());
    }

    if !remaining.is_empty() {
        println!("\nno font covers: {}", remaining.iter().collect::<String>());
    }
    Ok(())
}
