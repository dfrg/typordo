//! Tests for `<selectfont>`, the rules that decide what gets listed at all.

use std::path::{Path, PathBuf};

use fontconf::{Cache, Config, Object, Pattern};

fn fixture(name: &str) -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/select").join(name);
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cantarell() -> Cache {
    let path: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

fn with_first_font(cache: &Cache, f: impl FnOnce(Pattern<'_>)) {
    let font = cache.fonts().expect("fonts").next().expect("at least one font");
    f(font);
}

#[test]
fn a_config_without_selectfont_has_no_selectors() {
    let plain = Config::load_from(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/fonts.conf"),
    )
    .unwrap();
    assert!(!plain.has_selectors());
    // With no rules, everything is accepted.
    assert!(plain.accepts_filename("/anything/at/all.ttf"));
}

#[test]
fn reject_globs_exclude_and_accept_globs_override_them() {
    let config = fixture("fonts.conf");
    assert!(config.has_selectors());

    // Matched by a reject glob.
    assert!(!config.accepts_filename("/synthetic/fonts/bitmap/old.pcf"));
    assert!(!config.accepts_filename("/anywhere/fixed.pcf.gz"));
    // Named by an accept glob, which wins over the reject that also matches.
    assert!(config.accepts_filename("/synthetic/fonts/bitmap/keep.pcf"));
    // Named by neither, so accepted.
    assert!(config.accepts_filename("/synthetic/fonts/normal.ttf"));
}

/// A rejected directory prunes the walk, so the glob has to be applied to
/// subdirectory names too, not just to font files.
#[test]
fn a_reject_glob_applies_to_directories() {
    let config = fixture("fonts.conf");
    assert!(!config.accepts_filename("/synthetic/fonts/bitmap/sub"));
    assert!(config.accepts_filename("/synthetic/fonts/scalable"));
}

#[test]
fn a_pattern_selector_rejects_a_matching_font() {
    let config = fixture("reject-cantarell.conf");
    let cache = cantarell();
    with_first_font(&cache, |font| {
        assert_eq!(font.string(Object::Family), Some("Cantarell"));
        assert!(!config.accepts_font(&font));
        assert!(!config.accepts(&font));
    });
}

/// The fixture spells it `"  cantarell "`. Fontconfig compares selector
/// strings ignoring case and blanks, so that still matches.
#[test]
fn pattern_strings_ignore_case_and_blanks() {
    let config = fixture("reject-cantarell.conf");
    let cache = cantarell();
    with_first_font(&cache, |font| assert!(!config.accepts_font(&font)));
}

/// Every property a selector names must match. A selector naming a foundry
/// the font does not have matches nothing.
#[test]
fn a_selector_needs_all_of_its_properties_to_match() {
    let config = fixture("reject-unmatched.conf");
    let cache = cantarell();
    with_first_font(&cache, |font| {
        assert_eq!(font.string(Object::Family), Some("Cantarell"));
        assert!(config.accepts_font(&font), "one property matched, the other did not");
    });
}

#[test]
fn a_file_glob_rejects_through_the_combined_check() {
    let config = fixture("reject-by-file.conf");
    let cache = cantarell();
    with_first_font(&cache, |font| {
        // The pattern half allows it; the filename half does not.
        assert!(config.accepts_font(&font));
        assert!(!config.accepts(&font));
    });
}

/// Every pattern of the variable font shares one file, so a file-based
/// rejection removes all six instances rather than just the first.
#[test]
fn rejecting_a_file_removes_every_instance_in_it() {
    let config = fixture("reject-by-file.conf");
    let cache = cantarell();
    let total = cache.fonts().unwrap().count();
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(total, 6);
    assert_eq!(kept, 0);
}

// --- value kinds inside <patelt> ------------------------------------------

/// `<const>roman</const>` resolves to 0, and every Cantarell instance is
/// upright, so all six are rejected.
#[test]
fn a_const_resolves_to_its_numeric_value() {
    let config = fixture("const-slant.conf");
    let cache = cantarell();
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(kept, 0, "slant=roman should have matched every instance");
}

/// A selector this crate cannot fully evaluate must match nothing, rather
/// than being applied without the part it did not understand. Each of these
/// fixtures pairs a condition that *would* match with one that cannot be
/// evaluated; if the bad half were dropped, every font would be rejected.
#[test]
fn an_unevaluable_selector_never_matches() {
    let cache = cantarell();
    for name in ["const-unknown.conf", "langset-poison.conf", "unknown-object.conf"] {
        let config = fixture(name);
        let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
        assert_eq!(kept, 6, "{name} narrowed to its understood half and rejected fonts");
    }
}

/// Selector strings are compared with full Unicode case folding, not ASCII
/// lowercasing. Nothing on a typical system exercises this, so it is checked
/// directly rather than through a font.
#[test]
fn selector_strings_fold_beyond_ascii() {
    use fontconf::casefold;
    assert!(casefold::eq_ignoring_blanks("STRA\u{00df}E", "strasse"));
    // U+FB01 LATIN SMALL LIGATURE FI folds to "fi", and the blank is dropped.
    assert!(casefold::eq_ignoring_blanks("\u{fb01} le", "FILE"));
    assert!(casefold::eq_ignoring_blanks("\u{0391}\u{03a9}", "\u{03b1}\u{03c9}"));
}

/// `<const>` inside a `<patelt>` is looked up by name alone, so `normal` in a
/// `width` element resolves to the *weight* constant 80 that is declared
/// first. No font has width 80, so nothing is rejected. Resolving it per
/// property to 100 would be more sensible and would disagree with `fc-list`.
#[test]
fn a_const_ignores_the_property_it_appears_under() {
    let config = fixture("const-shadow.conf");
    let cache = cantarell();
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(kept, 6, "width=normal must resolve to weight 80 and match nothing");
}

/// `FcParseInt` uses `strtol` with base 0, so a codepoint written in hex is
/// normal in a config and a plain decimal parse would silently drop it.
#[test]
fn charset_codepoints_may_be_hex() {
    let config = fixture("charset-hex.conf");
    let cache = cantarell();
    // 0x41 is 'A', which Cantarell covers, so every instance is rejected.
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(kept, 0, "0x41 should have parsed as hex and matched");
}
