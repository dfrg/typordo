//! Scan a font file and print its properties, to compare with `fc-query`.
//!
//! ```text
//! cargo run --example fc_query -- --format family /path/to/font.ttf
//! ```

use fontconf::{Object, OwnedValue, Query};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut format = "family".to_string();
    let mut files: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => format = args.next().unwrap_or_default(),
            other => files.push(other.to_string()),
        }
    }
    let field = Object::from_name(&format)
        .ok_or_else(|| format!("unknown property {format}"))?;

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
            OwnedValue::Matrix(m) => format!("[{} {}; {} {}]", m.xx, m.xy, m.yx, m.yy),
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
