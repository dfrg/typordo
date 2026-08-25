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

use typordo::{
    render_prepare, CachePolicy, CharSet, Config, LangSet, Matrix, Object, Pattern, PatternRef,
    Range, Score, Tristate, Value, ValueType,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config_path: Option<PathBuf> = None;
    let mut format = "file".to_string();
    let mut score_of: Option<String> = None;
    let mut debug = false;
    let mut dump = false;
    let mut dump_match = false;
    let mut substitute = true;
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
            "--dump-match" => dump_match = true,
            // `fc-pattern` without `-c`: the name as parsed, before any
            // rule or default has touched it.
            "--no-substitute" => substitute = false,
            // Everything after `--` is a query, however it starts. A name
            // may begin with `-` -- that is how a bare size is written --
            // and `fc-pattern` needs the same separator to see it.
            "--" => terms.extend(args.by_ref()),
            other => terms.push(other.to_string()),
        }
    }

    let config = match &config_path {
        // fc-list and friends do not stop when a configuration will not
        // load: `FcInitLoadOwnConfig` runs on the built-in fallback
        // instead. Doing the same is what makes a comparison against
        // them meaningful when the config under test is a broken one.
        Some(path) => match Config::load_from(path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("cannot load {}: {e}", path.display());
                Config::fallback(None)?
            }
        },
        None => Config::load()?,
    };

    let caches: Vec<_> = config.caches(CachePolicy::read_only()).collect();
    let fonts: Vec<PatternRef<'_>> = caches
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
            let mut query = Pattern::new();
            parse_name(&mut query, line.trim_end())?;
            config.substitute(&mut query);
            query.default_substitute();
            answer(&config, &query, &fonts, field, sort);
        }
        return Ok(());
    }

    let mut query = Pattern::new();
    for term in &terms {
        parse_name(&mut query, term)?;
    }
    if substitute {
        config.substitute(&mut query);
        query.default_substitute();
    }

    if dump {
        for element in query.elements() {
            for (value, binding) in element.values() {
                println!("{}	{}	{}", element.object(), dumped(value), mark(binding));
            }
        }
        return Ok(());
    }

    // The prepared answer rather than the query: the same listing `fc-match
    // -v` prints, and the only way to see what binding matching settled on
    // for each object.
    if dump_match {
        if let Some((best, score)) = typordo::best(&query, fonts.to_vec()) {
            let prepared = render_prepare(&config, &query, &best, Some(&score));
            for element in prepared.elements() {
                for (value, binding) in element.values() {
                    println!("{}	{}	{}", element.object(), dumped(value), mark(binding));
                }
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
                if let Some(score) = typordo::score(&query, font) {
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
        for (font, score) in typordo::sorted(&query, fonts.clone()).iter().take(4) {
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
    query: &Pattern,
    fonts: &[PatternRef<'_>],
    field: Object,
    sort: Option<bool>,
) {
    match sort {
        Some(trim) => {
            for (font, _) in typordo::sort(query, fonts.to_vec(), trim) {
                let prepared = render_prepare(config, query, &font, None);
                println!("{}", show(&prepared, field));
            }
        }
        None => match typordo::best(query, fonts.to_vec()) {
            Some((best, score)) => {
                let prepared = render_prepare(config, query, &best, Some(&score));
                println!("{}", show(&prepared, field));
            }
            None => println!(),
        },
    }
}

/// A value as a harness can compare it.
///
/// `Debug` for everything whose type is worth seeing -- `Int(200)` and
/// `Double(200.0)` are a difference a name parser can get wrong -- but a
/// character or language set has to be spelled the way fontconfig spells it,
/// since `Debug` on those prints the bitmap.
fn dumped(value: &Value) -> String {
    match value {
        Value::CharSet(c) => format!("CharSet({})", typordo::AnyCharSet::Owned(c)),
        Value::LangSet(l) => format!("LangSet({})", typordo::AnyLangSet::Owned(l)),
        other => format!("{other:?}"),
    }
}

/// The letter `fc-match -v` suffixes a value with.
fn mark(binding: typordo::Binding) -> &'static str {
    match binding {
        typordo::Binding::Strong => "s",
        typordo::Binding::Weak => "w",
        typordo::Binding::Same => "?",
    }
}

/// Render one property the way `fc-match --format='%{field}'` does.
fn show(pattern: &Pattern, field: Object) -> String {
    let Some(element) = pattern.get(field) else {
        return String::new();
    };
    element
        .values()
        .map(|(value, _)| match value {
            Value::String(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Double(d) => format_g(*d),
            // `Tristate` prints the spellings fontconfig prints.
            Value::Bool(b) => b.to_string(),
            Value::Range(r) => format!("[{} {}]", format_g(r.begin), format_g(r.end)),
            Value::Matrix(m) => {
                format!("[{} {}; {} {}]", m.xx, m.xy, m.yx, m.yy)
            }
            Value::CharSet(c) => typordo::AnyCharSet::Owned(c).to_string(),
            Value::LangSet(l) => typordo::AnyLangSet::Owned(l).to_string(),
            Value::Void => String::new(),
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
fn parse_name(query: &mut Pattern, name: &str) -> Result<(), String> {
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
        // `FcNameFindNext (name, "=_:")`: either character separates a
        // property from its value, and `_` is not a rare spelling -- it is
        // what a name has to use where `=` would be taken for something else.
        let split = property.find(['=', '_']).map(|at| property.split_at(at));
        let Some((key, value)) = split.map(|(k, v)| (k, &v[1..])) else {
            // No separator, so the term is a bare constant and names its own
            // property: `:bold` is weight 200, `:italic` slant 100. Upstream
            // adds it as an **integer** whatever the property's declared type
            // is -- `FcPatternAddInteger` even for range-typed weight -- and
            // silently drops a word that is not a constant.
            if let Some((object, value)) = typordo::named_constant(property.trim()) {
                match object.value_type() {
                    ValueType::Bool => {
                        query.add(object, Tristate::from_i32(value));
                    }
                    ValueType::Int | ValueType::Double | ValueType::Range => {
                        query.add(object, value);
                    }
                    _ => {}
                }
            }
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

/// Add `value` to `object`, converted to the type the property is declared
/// to hold.
///
/// `FcNameConvert`. The text cannot say what it is on its own -- `True` is a
/// boolean for `scalable` and a family name for `family`, and `[10 20]` is a
/// range for `size` and a string anywhere else -- so the object's declared
/// type decides, not the shape of the text. Guessing instead gets `scalable`
/// wrong for every spelling but the lowercase one, and never produces a
/// range or a language set at all.
fn add_typed(query: &mut Pattern, object: Object, value: &str) {
    let value = value.trim();
    match object.value_type() {
        ValueType::Bool => {
            // `FcNameBool`, so `True`, `yes`, `on`, `1` and `dontcare` all work.
            query.add(object, Tristate::parse(value).unwrap_or(Tristate::False));
        }
        // A named constant first, then `atoi`, which yields 0 for anything
        // that is not a number at all. The constant has to belong to *this*
        // property: `slant=italic` is 100, and `slant=bold` is not 200.
        ValueType::Int => {
            let int =
                typordo::constant_for(object, value).or_else(|| leading_int(value)).unwrap_or(0);
            query.add(object, int);
        }
        // `strtod` and nothing else -- `FcNameConvert` looks up no constant
        // for a plain double, however tempting.
        ValueType::Double => {
            query.add(object, leading_double(value).unwrap_or(0.0));
        }
        ValueType::Range => {
            match parse_range(value) {
                Some(range) => {
                    query.add(object, range);
                }
                // `[light bold]` -- a range written with constants, which
                // both have to resolve for this property or the whole term
                // falls through to the scalar reading below.
                None => match parse_constant_range(object, value) {
                    Some(range) => {
                        query.add(object, range);
                    }
                    // A scalar reaches a range-typed property as a number,
                    // not as a one-point range. A word that is neither a
                    // constant for this property nor a number reaches it as
                    // nothing at all: upstream sets `FcTypeVoid`, so
                    // `:width=bold` adds no width rather than a wrong one.
                    None => {
                        if let Some(number) = typordo::constant_for(object, value)
                            .map(f64::from)
                            .or_else(|| whole_double(value))
                        {
                            query.add(object, number);
                        }
                    }
                },
            }
        }
        ValueType::LangSet => {
            let mut set = LangSet::new();
            for lang in value.split('|').filter(|l| !l.is_empty()) {
                set.insert(lang);
            }
            query.add(object, set);
        }
        // `FcNameParseCharSet`: space-separated hex codepoints, each on its
        // own or as a `first-last` range. One unreadable item and the whole
        // set is discarded, value and all.
        ValueType::CharSet => {
            if let Some(set) = parse_charset(value) {
                query.add(object, set);
            }
        }
        // `sscanf ("%lg %lg %lg %lg")` over an identity matrix, so a short
        // list leaves the rest of the identity in place.
        ValueType::Matrix => {
            let mut matrix = Matrix::IDENTITY;
            let mut numbers = value.split_whitespace().filter_map(|n| n.parse::<f64>().ok());
            for slot in [&mut matrix.xx, &mut matrix.xy, &mut matrix.yx, &mut matrix.yy] {
                match numbers.next() {
                    Some(number) => *slot = number,
                    None => break,
                }
            }
            query.add(object, matrix);
        }
        ValueType::String => {
            query.add(object, value);
        }
    }
}

/// `[begin end]`, the written form of a range.
fn parse_range(value: &str) -> Option<Range> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let (begin, end) = inner.split_once([' ', '-'])?;
    Some(Range { begin: begin.trim().parse().ok()?, end: end.trim().parse().ok()? })
}

/// `FcNameParseCharSet`: `41 42 43` or `41-43`, in hex.
fn parse_charset(value: &str) -> Option<CharSet> {
    let mut set = CharSet::new();
    for item in value.split_whitespace() {
        let (first, last) = match item.split_once('-') {
            Some((a, b)) => (a, b),
            None => (item, item),
        };
        let first = u32::from_str_radix(first, 16).ok()?;
        let last = u32::from_str_radix(last, 16).ok()?;
        for code in first..=last {
            set.insert(char::from_u32(code)?);
        }
    }
    Some(set)
}

/// `[light bold]`, a range written with two constants of this property.
///
/// Both have to resolve, and for *this* property -- `FcNameConvert` gives up
/// on the pair as soon as either does not, rather than mixing a constant with
/// a number.
fn parse_constant_range(object: Object, value: &str) -> Option<Range> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let (begin, end) = inner.split_once(' ')?;
    Some(Range {
        begin: typordo::constant_for(object, begin.trim())? as f64,
        end: typordo::constant_for(object, end.trim())? as f64,
    })
}

/// `atoi`: the leading integer, and nothing where there is not one.
///
/// Not `str::parse`, which rejects the trailing text `atoi` ignores.
fn leading_int(value: &str) -> Option<i32> {
    leading_double(value).map(|d| d as i32)
}

/// `strtod`: the leading number, ignoring whatever follows it.
fn leading_double(value: &str) -> Option<f64> {
    let text = value.trim_start();
    let mut end = 0;
    for (at, _) in text.char_indices() {
        if text[..=at].parse::<f64>().is_ok() {
            end = at + 1;
        }
    }
    (end > 0).then(|| text[..end].parse().unwrap())
}

/// `strtod` that has to consume the whole string, which is the check
/// `FcNameConvert` makes before deciding a range-typed value is unusable.
fn whole_double(value: &str) -> Option<f64> {
    value.trim().parse().ok()
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
