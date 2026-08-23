//! Time the operations fontconfig also does, for comparison with it.
//!
//! Paired with `scripts/bench_fc.c`, which links libfontconfig and runs the
//! same operations, and driven by `scripts/bench.sh`.
//!
//! ```text
//! cargo run --release --example bench -- load 1
//! cargo run --release --example bench -- match 200
//! ```
//!
//! # Two kinds of measurement
//!
//! `noop`, `config` and `load` are done **once per process**, because
//! fontconfig keeps every cache it has read in a process-wide table: asking
//! it to load them twice measures its memoisation against our real work. The
//! driver runs the whole binary many times instead and subtracts `noop`,
//! which is process start and dynamic linking.
//!
//! `list`, `match` and `sort` loop **inside** one process after loading once,
//! which is what both libraries actually do.

use std::time::Instant;

use fontconf::{best, sort as sort_fonts, Config, Object, OwnedValue, Pattern, Query};

/// The family a caller was already using when it ran out of coverage.
///
/// A fallback picker usually has one: the text was being set in something,
/// and the replacement should look like it where it can. Rotated so the
/// family half of scoring is exercised with names that exist here.
const HINTS: [&str; 4] = ["DejaVu Sans", "Liberation Serif", "Noto Sans", "Cantarell"];

/// A script, as a language tag and eight characters sampled from it.
///
/// This is the shape of query a fallback picker actually asks: it has some
/// text it cannot render, so it names the characters and the language, and
/// wants the font that covers them. No family at all.
const SCRIPTS: [(&str, [u32; 8]); 10] = [
    ("en", [0x41, 0x61, 0x7a, 0xe9, 0xf1, 0xfc, 0xdf, 0x152]),
    ("el", [0x3b1, 0x3b2, 0x3b3, 0x3b4, 0x3b5, 0x3b6, 0x3b7, 0x3b8]),
    ("ru", [0x430, 0x431, 0x432, 0x433, 0x434, 0x435, 0x436, 0x437]),
    ("he", [0x5d0, 0x5d1, 0x5d2, 0x5d3, 0x5d4, 0x5d5, 0x5d6, 0x5d7]),
    ("ar", [0x627, 0x628, 0x629, 0x62a, 0x62b, 0x62c, 0x62d, 0x62e]),
    ("hi", [0x905, 0x906, 0x907, 0x908, 0x909, 0x90a, 0x90b, 0x90c]),
    ("zh-cn", [0x4e00, 0x4e01, 0x4e02, 0x4e03, 0x4e04, 0x4e05, 0x4e06, 0x4e07]),
    ("ja", [0x3042, 0x3044, 0x3046, 0x3048, 0x304a, 0x304b, 0x304d, 0x304f]),
    ("ko", [0xac00, 0xac01, 0xac02, 0xac03, 0xac04, 0xac05, 0xac06, 0xac07]),
    ("th", [0xe01, 0xe02, 0xe03, 0xe04, 0xe05, 0xe06, 0xe07, 0xe08]),
];

/// The queries the match and sort benchmarks run, cycled through.
///
/// A mix on purpose: a family that exists, one that does not, a generic that
/// configuration has to expand, and a language request, because they take
/// very different paths through scoring.
const QUERIES: [&str; 8] = [
    "DejaVu Sans",
    "sans-serif",
    "serif:weight=200",
    "monospace",
    "NoSuchFamilyAnywhere",
    ":lang=ja",
    ":lang=en:weight=200:slant=100",
    "Noto Sans:lang=ar",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let op = args.next().unwrap_or_else(|| "noop".to_string());
    let iterations: u32 = args.next().unwrap_or_else(|| "1".into()).parse()?;

    let start = Instant::now();
    let checksum = run(&op, iterations)?;
    let elapsed = start.elapsed();

    // The checksum exists to stop the optimiser deleting the work; printing
    // it also catches a benchmark that quietly stopped doing anything.
    println!("{op} {iterations} {} {checksum}", elapsed.as_nanos());
    Ok(())
}

fn run(op: &str, iterations: u32) -> Result<u64, Box<dyn std::error::Error>> {
    match op {
        // Process start and dynamic linking, and nothing else.
        "noop" => Ok(0),

        // Parsing the configuration: 51 XML files here.
        "config" => {
            let config = Config::load()?;
            Ok(config.files().len() as u64)
        }

        // Configuration, then every cache, validated. This is the number a
        // program pays before it can answer anything.
        "load" => {
            let config = Config::load()?;
            let mut fonts = 0u64;
            for (_, cache) in config.caches() {
                cache.validate()?;
                fonts += cache.fonts()?.count() as u64;
            }
            Ok(fonts)
        }

        // Walking every font and touching three properties, which forces the
        // strings to be resolved rather than just counted.
        //
        // The duplicates are dropped, because `FcFontList` drops them: it
        // returns one entry per distinct combination of the properties asked
        // for, so `fc-list` prints fewer lines than there are patterns.
        // Without this the two sides would be listing different things.
        "list" => {
            let (config, caches) = loaded()?;
            let mut n = 0u64;
            for _ in 0..iterations {
                let mut seen = std::collections::HashSet::new();
                for (_, cache) in &caches {
                    for font in cache.fonts()? {
                        if !config.accepts(&font) {
                            continue;
                        }
                        let key = (
                            font.string(Object::Family).unwrap_or(""),
                            font.string(Object::File).unwrap_or(""),
                            font.string(Object::Style).unwrap_or(""),
                        );
                        if !seen.insert(key) {
                            continue;
                        }
                        n += (key.0.len() + key.1.len() + key.2.len()) as u64;
                    }
                }
            }
            Ok(n)
        }

        // One best-match per iteration, over the whole font set.
        "match" => {
            let (config, caches) = loaded()?;
            let fonts = fonts(&config, &caches)?;
            let mut n = 0u64;
            for i in 0..iterations {
                let query = prepared(&config, QUERIES[i as usize % QUERIES.len()]);
                if let Some((font, _score)) = best(&query, fonts.iter().copied()) {
                    n += font.string(Object::File).map_or(0, |s| s.len()) as u64;
                }
            }
            Ok(n)
        }

        // A full fallback list per iteration, which is the expensive one: it
        // scores every font and then trims by what each one adds.
        "sort" => {
            let (config, caches) = loaded()?;
            let fonts = fonts(&config, &caches)?;
            let mut n = 0u64;
            for i in 0..iterations {
                let query = prepared(&config, QUERIES[i as usize % QUERIES.len()]);
                n += sort_fonts(&query, fonts.iter().copied(), true).len() as u64;
            }
            Ok(n)
        }

        // What a fallback picker asks: a charset of a few characters and a
        // language, and no family at all. Every font in the set has to have
        // its coverage consulted, which none of the other queries do.
        "charmatch" | "charsort" | "hintmatch" | "hintsort" => {
            let (config, caches) = loaded()?;
            let fonts = fonts(&config, &caches)?;
            let mut n = 0u64;
            for i in 0..iterations {
                let (lang, chars) = &SCRIPTS[i as usize % SCRIPTS.len()];
                let mut query = Query::new();
                let mut coverage = fontconf::Coverage::new();
                for c in chars.iter().filter_map(|c| char::from_u32(*c)) {
                    coverage.insert(c);
                }
                query.add(Object::Charset, OwnedValue::CharSet(coverage));
                query.add(Object::Lang, *lang);
                if op.starts_with("hint") {
                    query.add(Object::Family, HINTS[i as usize % HINTS.len()]);
                }
                config.substitute(&mut query);
                query.default_substitute();

                if op.ends_with("sort") {
                    n += sort_fonts(&query, fonts.iter().copied(), true).len() as u64;
                } else if let Some((font, _)) = best(&query, fonts.iter().copied()) {
                    n += font.string(Object::File).map_or(0, |s| s.len()) as u64;
                }
            }
            Ok(n)
        }

        // Preparing a query and nothing else: parsing the name, running the
        // configuration over it, and applying the defaults. Both libraries
        // do this before they can match, so `match` includes it; this says
        // how much of that number it is.
        "prepare" => {
            let (config, _caches) = loaded()?;
            let mut n = 0u64;
            for i in 0..iterations {
                let query = prepared(&config, QUERIES[i as usize % QUERIES.len()]);
                n += query.len() as u64;
            }
            Ok(n)
        }

        // One query, repeated, so the cost of each kind can be seen apart.
        // Set QUERY to choose it.
        "matchq" => {
            let (config, caches) = loaded()?;
            let fonts = fonts(&config, &caches)?;
            let name = std::env::var("QUERY").unwrap_or_else(|_| "DejaVu Sans".into());
            let mut n = 0u64;
            for _ in 0..iterations {
                let query = prepared(&config, &name);
                if let Some((font, _score)) = best(&query, fonts.iter().copied()) {
                    n += font.string(Object::File).map_or(0, |s| s.len()) as u64;
                }
            }
            Ok(n)
        }

        // How many families and languages a prepared query carries, which is
        // what the scoring loop multiplies by the font count.
        "shape" => {
            let (config, _caches) = loaded()?;
            for name in QUERIES {
                let query = prepared(&config, name);
                let families = query.get(Object::Family).map_or(0, |e| e.values().count());
                let langs = query.get(Object::Lang).map_or(0, |e| e.values().count());
                let elements = query.len();
                eprintln!(
                    "  {name:<32} families={families:<4} langs={langs:<4} elements={elements}"
                );
            }
            Ok(0)
        }

        other => Err(format!("unknown operation {other}").into()),
    }
}

/// Everything a program holds once it can answer questions.
type Loaded = (Config, Vec<(String, fontconf::Cache)>);

/// Configuration and every cache, loaded once.
fn loaded() -> Result<Loaded, Box<dyn std::error::Error>> {
    let config = Config::load()?;
    let caches: Vec<_> = config.caches().collect();
    Ok((config, caches))
}

/// Every font the configuration accepts, as borrowed patterns.
fn fonts<'a>(
    config: &Config,
    caches: &'a [(String, fontconf::Cache)],
) -> Result<Vec<Pattern<'a>>, Box<dyn std::error::Error>> {
    Ok(caches
        .iter()
        .filter_map(|(_, cache)| cache.fonts().ok())
        .flatten()
        .filter(|font| config.accepts(font))
        .collect())
}

/// A query with configuration and defaults applied, as matching expects.
fn prepared(config: &Config, name: &str) -> Query {
    let mut query = Query::new();
    parse(&mut query, name);
    config.substitute(&mut query);
    query.default_substitute();
    query
}

/// Enough of `FcNameParse` for the benchmark queries: a family, then
/// `:key=value` terms.
fn parse(query: &mut Query, name: &str) {
    let mut parts = name.split(':');
    if let Some(family) = parts.next() {
        if !family.is_empty() {
            query.add(Object::Family, family);
        }
    }
    for term in parts {
        let Some((key, value)) = term.split_once('=') else { continue };
        let Some(object) = Object::from_name(key) else { continue };
        match value.parse::<i32>() {
            Ok(number) => query.add(object, number),
            Err(_) => query.add(object, value),
        };
    }
}
