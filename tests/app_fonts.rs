//! Fonts an application supplies itself, alongside the system's.
//!
//! `FcConfigAppFontAddFile` puts them in a second font set and
//! `FcFontMatch` walks system first, application second. This crate has no
//! second set because matching takes an *iterator* of fonts rather than a
//! configuration -- so the caller decides both what is considered and in what
//! order, and an application font set is a chained iterator.
//!
//! What that needs is a way to get from owned patterns, which is what
//! scanning produces, to the borrowed ones matching scores.
//! [`Cache::from_fonts`] is that bridge, and these tests are here to show it
//! holds.

use std::path::Path;

use typordo::{best, sorted, Cache, Object, Pattern, PatternRef, Value};

fn system() -> Cache {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

/// Owned patterns as a cache that can be matched against, which is what
/// `FcConfigAppFontAddFile` produces internally.
fn app_font_set(dir: &str, fonts: &[Pattern]) -> Cache {
    Cache::from_fonts(dir, fonts).expect("a cache we just built")
}

/// One pattern from the fixture, renamed, standing in for a font an
/// application ships.
fn renamed(cache: &Cache, family: &str, file: &str) -> Pattern {
    let font = cache.fonts().unwrap().next().expect("a font");
    let mut pattern = Pattern::from_pattern(&font);
    // Replace rather than append: `add` would leave the original family in
    // front of the new one.
    pattern.remove(Object::Family);
    pattern.add(Object::Family, family);
    pattern.remove(Object::File);
    pattern.add(Object::File, file);
    pattern
}

fn family_of(font: &PatternRef<'_>) -> String {
    font.string(Object::Family).unwrap_or_default().to_string()
}

/// The whole question: can a caller put its own fonts in front of the
/// system's and have them matched? They can, with no API this crate does not
/// already have.
#[test]
fn an_application_font_can_be_matched_alongside_the_system() {
    let system = system();
    let app = app_font_set("/app/fonts", &[renamed(&system, "Bundled Sans", "/app/fonts/b.otf")]);

    let mut query = Pattern::new();
    query.add(Object::Family, "Bundled Sans");

    // System alone cannot answer: nothing is called that.
    let (font, _) = best(&query, system.fonts().unwrap()).expect("something always matches");
    assert_ne!(family_of(&font), "Bundled Sans");

    // Chained, it does -- and the file is the application's.
    let chained = system.fonts().unwrap().chain(app.fonts().unwrap());
    let (font, _) = best(&query, chained).expect("a match");
    assert_eq!(family_of(&font), "Bundled Sans");
    assert_eq!(font.string(Object::File), Some("/app/fonts/b.otf"));
}

/// Order decides a tie, and the caller controls the order. Fontconfig walks
/// system first and requires a *strictly* better score to replace the
/// incumbent, so a system font wins a tie against an application font.
/// Chaining the other way round is how a caller gets the opposite, which
/// `FcConfigAppFontAddFile` gives no way to ask for.
#[test]
fn the_chain_order_decides_a_tie() {
    let system = system();
    let name = family_of(&system.fonts().unwrap().next().unwrap());
    // Same family as the system font, so the two score identically.
    let app = app_font_set("/app/fonts", &[renamed(&system, &name, "/app/fonts/same.otf")]);

    let mut query = Pattern::new();
    query.add(Object::Family, name.as_str());

    let system_first = system.fonts().unwrap().chain(app.fonts().unwrap());
    let (font, _) = best(&query, system_first).expect("a match");
    assert_ne!(
        font.string(Object::File),
        Some("/app/fonts/same.otf"),
        "fontconfig's order: the incumbent keeps a tie"
    );

    let app_first = app.fonts().unwrap().chain(system.fonts().unwrap());
    let (font, _) = best(&query, app_first).expect("a match");
    assert_eq!(
        font.string(Object::File),
        Some("/app/fonts/same.otf"),
        "the caller can have it the other way round"
    );
}

/// The same for a whole fallback chain, which is what a text layout engine
/// actually walks.
#[test]
fn an_application_font_takes_its_place_in_the_sort() {
    let system = system();
    let app = app_font_set("/app/fonts", &[renamed(&system, "Bundled Sans", "/app/fonts/b.otf")]);

    let mut query = Pattern::new();
    query.add(Object::Family, "Bundled Sans");

    let chained: Vec<PatternRef<'_>> =
        system.fonts().unwrap().chain(app.fonts().unwrap()).collect();
    let order = sorted(&query, chained);
    assert!(order.len() > 1, "the fixture has several faces");
    assert_eq!(family_of(&order[0].0), "Bundled Sans", "the exact family sorts first");
}

/// Everything else about the pattern survives the round trip into a cache,
/// so an application font is not a second-class one.
#[test]
fn an_application_font_keeps_its_properties() {
    let system = system();
    let source = system.fonts().unwrap().next().unwrap();
    let owned = Pattern::from_pattern(&source);
    let app = app_font_set("/app/fonts", std::slice::from_ref(&owned));

    let read_back = app.fonts().unwrap().next().expect("the font we wrote");
    for object in [Object::Family, Object::Style, Object::Weight, Object::Slant, Object::Charset] {
        let before = owned.value(object).cloned();
        let after = read_back.value(object).map(|v| Value::from_value(&v));
        assert_eq!(before, after, "{object} did not survive");
    }
}

/// `Cache::from_fonts` takes anything that yields patterns, not just a slice,
/// so a caller need not collect first.
#[test]
fn from_fonts_accepts_any_iterator_of_patterns() {
    let system = system();
    let owned: Vec<Pattern> =
        system.fonts().unwrap().map(|font| Pattern::from_pattern(&font)).collect();

    let from_slice = Cache::from_fonts("/app/fonts", &owned).unwrap();
    let from_iter = Cache::from_fonts("/app/fonts", owned.iter().take(1)).unwrap();

    assert_eq!(from_slice.fonts().unwrap().count(), owned.len());
    assert_eq!(from_iter.fonts().unwrap().count(), 1);
    assert_eq!(from_slice.dir().unwrap(), "/app/fonts");
}

/// An empty set is a cache with no fonts, not an error: an application that
/// bundles none should not have to special-case that.
#[test]
fn from_fonts_accepts_nothing_at_all() {
    let cache = Cache::from_fonts("/app/fonts", &[]).expect("an empty cache is still a cache");
    assert_eq!(cache.fonts().unwrap().count(), 0);
    assert_eq!(cache.dir().unwrap(), "/app/fonts");
}
