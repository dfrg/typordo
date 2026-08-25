//! Turning a chosen font back into a pattern the caller can use.
//!
//! Matching picks a font, but the answer is not the font's cache entry: it is
//! a merge of the font and the query. Properties the font has are narrowed to
//! the single value that answered best; properties only the query had are
//! carried over, which is how `dpi`, `hintstyle` and the rest survive into the
//! result. Then the configuration's `target="font"` rules get a turn.

use crate::config::Config;
use crate::matching;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::pattern::PatternRef;
use crate::rules::MatchKind;
use crate::value::Value;
use crate::value::{Binding, ValueRef};

/// The three name properties that travel with a parallel language list.
///
/// Fontconfig relies on each sitting immediately before its `*lang`
/// counterpart in the object numbering; here they are paired explicitly.
const LOCALIZED: [(Object, Object); 3] = [
    (Object::Family, Object::Familylang),
    (Object::Style, Object::Stylelang),
    (Object::Fullname, Object::Fullnamelang),
];

fn lang_counterpart(object: Object) -> Option<Object> {
    LOCALIZED.iter().find(|(name, _)| *name == object).map(|(_, lang)| *lang)
}

fn is_lang_counterpart(object: Object) -> bool {
    LOCALIZED.iter().any(|(_, lang)| *lang == object)
}

/// Build the pattern that answers `query`, given the font that won.
///
/// This is `FcFontRenderPrepare`. Call it with the query *after*
/// [`Config::substitute`] and [`Pattern::default_substitute`] have run, since
/// those are what the font is merged against.
pub fn render_prepare(config: &Config, query: &Pattern, font: &PatternRef<'_>) -> Pattern {
    let mut out = Pattern::new();
    let variable = font.value(Object::Variable).and_then(|v| v.as_bool()) == Some(true);
    let mut variations: Vec<String> = Vec::new();

    for element in font.elements() {
        let Some(object) = element.object() else { continue };
        // A language list is emitted with the name it belongs to, never alone.
        if is_lang_counterpart(object) {
            continue;
        }

        if let Some(lang_object) = lang_counterpart(object) {
            if font.get(lang_object).is_some() {
                copy_localized(query, font, object, lang_object, &mut out);
                continue;
            }
        }

        match query.get(object) {
            // Both sides have it: the result keeps the one value that won.
            Some(_) => {
                let best = matching::best_value(query, font, object);
                let index = best.as_ref().map_or(0, |b| b.index);
                let resolved = best.as_ref().and_then(|b| b.resolved);
                let value = match resolved {
                    Some(number) => Some(Value::Double(number)),
                    None => element.values().nth(index).map(own),
                };
                if let Some(value) = value {
                    // A variable font records the axis it was pinned to, so a
                    // renderer can instantiate the same instance we scored.
                    if variable && matches!(element.values().next(), Some(ValueRef::Range(_))) {
                        if let Some(tag) = axis_tag(object) {
                            if let Some(number) = resolved {
                                variations.push(format_axis(tag, object, number));
                            }
                        }
                    }
                    out.add(object, value);
                }
            }
            // Only the font has it: carry the whole list across.
            None => {
                for (value, binding) in element.values().zip(bindings(element)) {
                    out.add_with_binding(object, own(value), binding);
                }
            }
        }
    }

    // Anything the query asked for that the font never mentions -- dpi,
    // hintstyle, the rendering hints -- would otherwise be lost.
    for element in query.elements() {
        let object = element.object();
        if font.get(object).is_some() || is_lang_counterpart(object) {
            continue;
        }
        for (value, binding) in element.values() {
            out.add_with_binding(object, value.clone(), binding);
        }
    }

    if variable && !variations.is_empty() {
        if let Some(existing) = out.string(Object::FontVariations) {
            variations.push(existing.to_string());
        }
        let joined = variations.join(",");
        out.remove(Object::FontVariations);
        out.add(Object::FontVariations, joined);
    }

    // Font-target rules run last, and can see the original query.
    config.substitute_kind(&mut out, MatchKind::Font, Some(query));
    out
}

/// Copy a localized name and its language list, promoting the language the
/// query asked for.
///
/// When the query names languages of its own, the entry whose language scored
/// best is moved to the front and made strong, so `%{family}` reports the
/// caller's preferred localization rather than whichever the font listed
/// first. Without a language preference the lists are copied unchanged.
fn copy_localized(
    query: &Pattern,
    font: &PatternRef<'_>,
    name: Object,
    lang: Object,
    out: &mut Pattern,
) {
    let names: Vec<Value> =
        font.get(name).map(|e| e.values().map(own).collect()).unwrap_or_default();
    let langs: Vec<Value> =
        font.get(lang).map(|e| e.values().map(own).collect()).unwrap_or_default();

    // The query's own language list decides which entry to promote.
    let promote = query
        .get(lang)
        .and_then(|_| matching::best_value(query, font, lang))
        .map(|best| best.index);

    let order = |values: &[Value]| -> Vec<usize> {
        let mut order: Vec<usize> = (0..values.len()).collect();
        if let Some(at) = promote {
            if at < order.len() {
                let picked = order.remove(at);
                order.insert(0, picked);
            }
        }
        order
    };

    for (position, index) in order(&names).into_iter().enumerate() {
        let binding =
            if position == 0 && promote.is_some() { Binding::Strong } else { Binding::Weak };
        out.add_with_binding(name, names[index].clone(), binding);
    }
    for (position, index) in order(&langs).into_iter().enumerate() {
        let binding =
            if position == 0 && promote.is_some() { Binding::Strong } else { Binding::Weak };
        out.add_with_binding(lang, langs[index].clone(), binding);
    }
}

/// The OpenType axis a property pins on a variable font.
fn axis_tag(object: Object) -> Option<&'static str> {
    match object {
        Object::Weight => Some("wght"),
        Object::Width => Some("wdth"),
        Object::Size => Some("opsz"),
        _ => None,
    }
}

/// Format one axis setting the way fontconfig writes `fontvariations`.
///
/// Weight is the odd one: the value is fontconfig's own 0..215 scale and has
/// to be converted back to OpenType's 1..1000 before it means anything to a
/// font.
fn format_axis(tag: &str, object: Object, value: f64) -> String {
    let number = if object == Object::Weight { crate::weight::to_opentype(value) } else { value };
    format!("{tag}={}", format_number(number))
}

/// `%g`-style formatting, which is what fontconfig uses here.
fn format_number(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let text = format!("{value:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn own(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Void => Value::Void,
        ValueRef::Int(i) => Value::Int(i),
        ValueRef::Double(d) => Value::Double(d),
        ValueRef::String(s) => Value::String(s.to_string()),
        ValueRef::Bool(b) => Value::Bool(b),
        ValueRef::Matrix(m) => Value::Matrix(m),
        ValueRef::Range(r) => Value::Range(r),
        ValueRef::CharSet(chars) => {
            let mut coverage = crate::charset::CharSet::new();
            for c in chars.chars() {
                coverage.insert(c);
            }
            Value::CharSet(coverage)
        }
        ValueRef::LangSet(langs) => {
            let mut owned = crate::langset::LangSet::new();
            for index in 0..crate::langs::LANGS.len() {
                if langs.contains_index(index) {
                    owned.insert_index(index);
                }
            }
            Value::LangSet(owned)
        }
    }
}

/// The bindings of a font element's values.
fn bindings<'a>(element: crate::pattern::ElementRef<'a>) -> impl Iterator<Item = Binding> + 'a {
    element.values().bindings().map(|(_, binding)| binding)
}
