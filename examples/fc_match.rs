//! Pick the best font for a query, to compare with `fc-match`.
//!
//! ```text
//! cargo run --example fc_match -- --config /tmp/plain.conf "DejaVu Sans"
//! cargo run --example fc_match -- --config /tmp/plain.conf "DejaVu Sans:weight=200"
//! cargo run --example fc_match -- --config /tmp/plain.conf --score-of FILE "query"
//! ```
//!
//! Only `<dir>`, `<cachedir>` and `<selectfont>` are read, so pointing this at
//! a real `/etc/fonts/fonts.conf` will disagree with `fc-match`: the config's
//! `<match>` rules rewrite the query first, and none of that happens yet.

use std::path::PathBuf;

use fontconf::{Config, Object, Pattern, Query, Score};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config_path: Option<PathBuf> = None;
    let mut format = "file".to_string();
    let mut score_of: Option<String> = None;
    let mut debug = false;
    let mut terms: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            "--format" => format = args.next().unwrap_or_default(),
            "--score-of" => score_of = args.next(),
            "--debug" => debug = true,
            other => terms.push(other.to_string()),
        }
    }

    let config = match &config_path {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };

    let mut query = Query::new();
    for term in &terms {
        parse_name(&mut query, term)?;
    }
    query.default_substitute();

    let caches: Vec<_> = config.caches().collect();
    let fonts: Vec<Pattern<'_>> = caches
        .iter()
        .filter_map(|(_, cache)| cache.fonts().ok())
        .flatten()
        .filter(|font| config.accepts(font))
        .collect();

    // Report our own score for one specific file, so a harness can tell
    // "we picked a worse font" apart from "the two fonts scored identically
    // and fontconfig's tie-break differs from ours".
    if let Some(wanted) = &score_of {
        for font in &fonts {
            if font.string(Object::File) == Some(wanted.as_str()) {
                if let Some(score) = fontconf::score(&query, font) {
                    println!("{}", format_score(&score));
                    return Ok(());
                }
            }
        }
        return Err(format!("no font with file {wanted}").into());
    }

    if debug {
        eprintln!("query: {query}");
        for (font, score) in fontconf::sorted(&query, fonts.clone()).iter().take(4) {
            eprintln!("  {}", font.string(Object::File).unwrap_or(""));
            eprintln!("      {}", format_score(score));
        }
    }

    let Some((best, _)) = fontconf::best(&query, fonts) else {
        return Err("no font matched".into());
    };

    let field = match format.as_str() {
        "family" => Object::Family,
        "style" => Object::Style,
        _ => Object::File,
    };
    println!("{}", best.string(field).unwrap_or(""));
    Ok(())
}

fn format_score(score: &Score) -> String {
    score
        .as_slice()
        .iter()
        .map(|v| format!("{v:.6e}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a font name the way `FcNameParse` does.
///
/// Families come first, separated by commas. A `-` ends the family list and
/// starts a size list; a `:` ends either and starts `name=value` properties.
/// A backslash escapes the next character anywhere.
fn parse_name(query: &mut Query, name: &str) -> Result<(), String> {
    let (families, delim, rest) = take_until(name, "-,:");
    let mut families = vec![families];
    let mut delim = delim;
    let mut rest = rest;
    while delim == Some(',') {
        let (next, d, r) = take_until(rest, "-,:");
        families.push(next);
        delim = d;
        rest = r;
    }
    for family in families.into_iter().filter(|f| !f.is_empty()) {
        query.add(Object::Family, family.as_str());
    }

    // Sizes, if a '-' introduced them. A size that is not a number is simply
    // ignored, which is why "DejaVuSans-Bold" is a family and nothing else.
    if delim == Some('-') {
        loop {
            let (text, d, r) = take_until(rest, "-,:");
            if let Ok(size) = text.trim().parse::<f64>() {
                query.add(Object::Size, size);
            }
            delim = d;
            rest = r;
            if delim != Some(',') {
                break;
            }
        }
    }

    while delim == Some(':') {
        let (property, d, r) = take_until(rest, ":");
        delim = d;
        rest = r;
        let Some((key, value)) = property.split_once('=') else {
            continue;
        };
        let object = Object::from_name(key.trim())
            .ok_or_else(|| format!("unknown property {key}"))?;
        for value in value.split(',') {
            add_typed(query, object, value);
        }
    }
    Ok(())
}

/// Add `value` to `object`, inferring its type from how it is written.
fn add_typed(query: &mut Query, object: Object, value: &str) {
    if let Ok(int) = value.parse::<i32>() {
        query.add(object, int);
    } else if let Ok(double) = value.parse::<f64>() {
        query.add(object, double);
    } else if value == "true" || value == "false" {
        query.add(object, value == "true");
    } else {
        query.add(object, value);
    }
}

/// Read up to the first unescaped character in `delims`.
///
/// Returns the text read with escapes resolved, the delimiter that stopped it,
/// and the remainder after that delimiter.
fn take_until<'a>(input: &'a str, delims: &str) -> (String, Option<char>, &'a str) {
    let mut out = String::new();
    let mut chars = input.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            if let Some((_, escaped)) = chars.next() {
                out.push(escaped);
            }
            continue;
        }
        if delims.contains(c) {
            return (out, Some(c), &input[i + c.len_utf8()..]);
        }
        out.push(c);
    }
    (out, None, "")
}
