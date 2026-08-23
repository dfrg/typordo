//! Pick the best font for a query, to compare with `fc-match`.
//!
//! ```text
//! cargo run --example fc_match -- --config /tmp/plain.conf "DejaVu Sans"
//! cargo run --example fc_match -- --config /tmp/plain.conf "DejaVu Sans:weight=200"
//! cargo run --example fc_match -- --config /tmp/plain.conf --score-of FILE "query"
//! ```
//!
//! The query is rewritten by the config's `<match>` rules before scoring,
//! the same order fontconfig uses: substitution first, then the defaults.

use std::path::PathBuf;

use fontconf::{render_prepare, Config, Object, OwnedValue, Pattern, Query, Score};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config_path: Option<PathBuf> = None;
    let mut format = "file".to_string();
    let mut score_of: Option<String> = None;
    let mut debug = false;
    let mut dump = false;
    let mut batch = false;
    // Some(true) = sorted and trimmed (-s), Some(false) = sorted, untrimmed (-a).
    let mut sort: Option<bool> = None;
    let mut terms: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            "--format" => format = args.next().unwrap_or_default(),
            "--score-of" => score_of = args.next(),
            "--batch" => batch = true,
            "--sort" => sort = Some(true),
            "--all" => sort = Some(false),
            "--debug" => debug = true,
            "--dump-query" => dump = true,
            other => terms.push(other.to_string()),
        }
    }

    let config = match &config_path {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };

    let caches: Vec<_> = config.caches().collect();
    let fonts: Vec<Pattern<'_>> = caches
        .iter()
        .filter_map(|(_, cache)| cache.fonts().ok())
        .flatten()
        .filter(|font| config.accepts(font))
        .collect();

    let field = Object::from_name(&format).ok_or_else(|| format!("unknown property {format}"))?;

    // One query per line on stdin, one answer per line out. Loading every
    // cache costs more than matching does, so a harness running hundreds of
    // queries should pay for it once.
    if batch {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            let mut query = Query::new();
            parse_name(&mut query, line.trim_end())?;
            config.substitute(&mut query);
            query.default_substitute();
            answer(&config, &query, &fonts, field, sort);
        }
        return Ok(());
    }

    let mut query = Query::new();
    for term in &terms {
        parse_name(&mut query, term)?;
    }
    config.substitute(&mut query);
    query.default_substitute();

    if dump {
        for element in query.elements() {
            for (value, binding) in element.values() {
                let mark = match binding {
                    fontconf::Binding::Strong => "s",
                    fontconf::Binding::Weak => "w",
                    fontconf::Binding::Same => "?",
                };
                println!("{}	{value:?}	{mark}", element.object());
            }
        }
        return Ok(());
    }

    // Report our own score for one specific file, so a harness can tell
    // "we picked a worse font" apart from "the two fonts scored identically
    // and fontconfig's tie-break differs from ours".
    if let Some(wanted) = &score_of {
        // Every pattern for the file, not the first: a variable font
        // contributes one per named instance, and they score differently.
        for font in &fonts {
            if font.string(Object::File) == Some(wanted.as_str()) {
                if let Some(score) = fontconf::score(&query, font) {
                    println!(
                        "weight={:<10} instance={:<6} {}",
                        font.value(Object::Weight).map_or("?".to_string(), |v| format!("{v:?}")),
                        font.value(Object::NamedInstance)
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        format_score(&score)
                    );
                    continue;
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

    answer(&config, &query, &fonts, field, sort);
    Ok(())
}

/// Print the answer, either one font or a whole sorted list.
///
/// `fc-match` runs every entry of a sort through render_prepare too, not just
/// the winner, so the same has to happen here.
fn answer(
    config: &Config,
    query: &Query,
    fonts: &[Pattern<'_>],
    field: Object,
    sort: Option<bool>,
) {
    match sort {
        Some(trim) => {
            for (font, _) in fontconf::sort(query, fonts.to_vec(), trim) {
                let prepared = render_prepare(config, query, &font);
                println!("{}", show(&prepared, field));
            }
        }
        None => match fontconf::best(query, fonts.to_vec()) {
            Some((best, _)) => {
                let prepared = render_prepare(config, query, &best);
                println!("{}", show(&prepared, field));
            }
            None => println!(),
        },
    }
}

/// Render one property the way `fc-match --format='%{field}'` does.
fn show(pattern: &Query, field: Object) -> String {
    let Some(element) = pattern.get(field) else {
        return String::new();
    };
    element
        .values()
        .map(|(value, _)| match value {
            OwnedValue::String(s) => s.clone(),
            OwnedValue::Int(i) => i.to_string(),
            OwnedValue::Double(d) => format_g(*d),
            OwnedValue::Bool(b) => if *b { "True" } else { "False" }.to_string(),
            OwnedValue::Range(r) => format!("[{} {}]", format_g(r.begin), format_g(r.end)),
            OwnedValue::Matrix(m) => {
                format!("[{} {}; {} {}]", m.xx, m.xy, m.yx, m.yy)
            }
            OwnedValue::CharSet(c) => fontconf::Chars::Owned(c).to_string(),
            OwnedValue::LangSet(l) => fontconf::Languages::Owned(l).to_string(),
            OwnedValue::Void => String::new(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// C's `%g`: six significant digits, no trailing zeroes.
fn format_g(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).max(0) as usize;
    let text = format!("{value:.decimals$}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn format_score(score: &Score) -> String {
    score.as_slice().iter().map(|v| format!("{v:.6e}")).collect::<Vec<_>>().join(" ")
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
        let object =
            Object::from_name(key.trim()).ok_or_else(|| format!("unknown property {key}"))?;
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
