//! Pick a font for some text, the way an application would.
//!
//! The question fontconfig exists to answer: given a generic family, some
//! characters and a language, which installed font should render it?
//!
//! ```text
//! cargo run --example pick_font -- sans-serif ja こんにちは
//! cargo run --example pick_font -- serif ar "مرحبا"
//! cargo run --example pick_font -- monospace en "fn main()"
//! ```

use std::error::Error;

use fontconf::{
    best, render_prepare, Config, Coverage, Object, OwnedValue, Pattern, Priority, Query,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (family, lang, text) = match args.as_slice() {
        [family, lang, text] => (family.as_str(), lang.as_str(), text.as_str()),
        _ => {
            eprintln!("usage: pick_font <family> <lang> <text>");
            std::process::exit(2);
        }
    };

    // The configuration says where fonts live, which caches to read, and what
    // rules rewrite a query. `Config::load()` finds it the way fontconfig
    // does, starting at `/etc/fonts/fonts.conf`.
    let config = Config::load()?;

    // Caches own their bytes and patterns borrow from them, so the caches
    // have to outlive the fonts. `accepts` applies the config's <selectfont>
    // rules, which is how a system hides a font without uninstalling it.
    let caches: Vec<_> = config.caches().collect();
    let fonts: Vec<Pattern<'_>> = caches
        .iter()
        .filter_map(|(_, cache)| cache.fonts().ok())
        .flatten()
        .filter(|font| config.accepts(font))
        .collect();
    println!("{} fonts in {} caches", fonts.len(), caches.len());

    // What we want. A charset asks for coverage of specific characters, which
    // is what makes this a fallback query rather than a name lookup: no font
    // is called "こんにちは", but some font covers it.
    let mut query = Query::new();
    query.add(Object::Family, family);
    query.add(Object::Lang, lang);

    let mut wanted = Coverage::new();
    for c in text.chars() {
        wanted.insert(c);
    }
    query.add(Object::Charset, OwnedValue::CharSet(wanted));

    // Both rewrites, in this order. The first resolves `sans-serif` into the
    // real families the configuration prefers; the second fills in the
    // weight, slant and size the query never mentioned. Matching assumes
    // both have run.
    config.substitute(&mut query);
    query.default_substitute();

    let Some((font, score)) = best(&query, fonts.iter().cloned()) else {
        println!("no font matched");
        return Ok(());
    };

    // The winner is a pattern in the cache, describing the whole font. What
    // an application wants is the query answered *by* that font: the family
    // it matched under, the size that was asked for, the localized name that
    // fit. That merge is `FcFontRenderPrepare`.
    let prepared = render_prepare(&config, &query, &font);

    println!();
    println!("family    {}", prepared.string(Object::Family).unwrap_or("?"));
    println!("style     {}", prepared.string(Object::Style).unwrap_or("?"));
    println!("file      {}", prepared.string(Object::File).unwrap_or("?"));
    if let Some(index) = prepared.number(Object::Index) {
        println!("index     {index}");
    }

    // A match is always returned, even a bad one -- fontconfig ranks fonts,
    // it does not reject them. Whether the answer is usable is a question
    // about coverage, and it is the caller's to ask.
    let missing: Vec<char> = match font.value(Object::Charset) {
        Some(fontconf::Value::CharSet(set)) => text.chars().filter(|c| !set.contains(*c)).collect(),
        _ => text.chars().collect(),
    };
    if missing.is_empty() {
        println!("coverage  all {} characters", text.chars().count());
    } else {
        println!("coverage  MISSING {missing:?}");
    }

    // The score is a vector of distances, one per priority, compared
    // lexicographically -- so an earlier slot outranks every later one no
    // matter how large they get. Zero means the font answered that part of
    // the query exactly. These four are the ones this query is about.
    println!();
    for priority in
        [Priority::CharSet, Priority::Lang, Priority::FamilyStrong, Priority::FamilyWeak]
    {
        let distance = score.get(priority);
        // A generic family resolves into aliases the configuration prefers,
        // and those are weak bindings -- so `FamilyStrong` staying at its
        // no-match sentinel here is the normal outcome, not a failure.
        let shown = if distance >= 1e99 {
            "no match".to_string()
        } else if distance == 0.0 {
            "exact".to_string()
        } else {
            distance.to_string()
        };
        // A derived `Debug` ignores width, so the name is padded as a string.
        println!("  {:<14} {shown}", format!("{priority:?}"));
    }
    Ok(())
}
