//! Scan a font file and print its properties, to compare with `fc-query`.
//!
//! ```text
//! cargo run --example fc_query -- --format family /path/to/font.ttf
//! ```

use fontconf::{Object, OwnedValue, Query};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut format = "family".to_string();
    let mut batch = false;
    let mut files: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => format = args.next().unwrap_or_default(),
            // One path per line on stdin. Scanning is fast; starting a
            // process per file is not, and a harness runs thousands.
            "--batch" => batch = true,
            other => files.push(other.to_string()),
        }
    }
    // `properties` asks which properties a pattern has rather than what one
    // of them says. An element that exists with an empty value prints the
    // same as one that is absent, and the two score differently, so the only
    // way to compare them is to compare the names.
    let field = if format == "properties" {
        None
    } else {
        Some(
            Object::from_name(&format)
                .ok_or_else(|| format!("unknown property {format}"))?,
        )
    };

    if batch {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let file = line?;
            let file = file.trim_end();
            match fontconf::scan_file(std::path::Path::new(file)) {
                Ok(patterns) => {
                    // A marker per file, so a harness can split the stream
                    // back into per-file answers.
                    println!("@{file}");
                    for pattern in patterns {
                        println!("{}", show(&pattern, field));
                    }
                }
                Err(e) => {
                    println!("@{file}");
                    eprintln!("{file}: {e}");
                }
            }
        }
        return Ok(());
    }

    for file in &files {
        match fontconf::scan_file(std::path::Path::new(file)) {
            Ok(patterns) => {
                for pattern in patterns {
                    println!("{}", show(&pattern, field));
                }
            }
            // fc-query prints nothing and fails; match that rather than
            // inventing an empty line, so counts stay comparable.
            Err(e) => eprintln!("{file}: {e}"),
        }
    }
    Ok(())
}

fn show(pattern: &Query, field: Option<Object>) -> String {
    let Some(field) = field else {
        // The property names, in the order the pattern holds them.
        return pattern
            .elements()
            .map(|element| element.object().name().to_string())
            .collect::<Vec<_>>()
            .join(",");
    };
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
            OwnedValue::Matrix(m) => format!("[{} {}; {} {}]", m.xx, m.xy, m.yx, m.yy),
            OwnedValue::CharSet(c) => fontconf::Chars::Owned(c).to_string(),
            OwnedValue::LangSet(l) => fontconf::Languages::Owned(l).to_string(),
            OwnedValue::Void => String::new(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_g(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).max(0) as usize;
    let text = format!("{value:.decimals$}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
