//! Tests for `<selectfont>`, the rules that decide what gets listed at all.

use std::path::{Path, PathBuf};

use fontconf::{Cache, Config, Object, Pattern};

fn fixture(name: &str) -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/select")
        .join(name);
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cantarell() -> Cache {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cantarell-le64.cache-9");
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
