//! Tests for `<selectfont>`, the rules that decide what gets listed at all.

use std::path::{Path, PathBuf};

use typordo::{Cache, Config, Object, PatternRef};

fn fixture(name: &str) -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/select").join(name);
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cantarell() -> Cache {
    let path: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

fn with_first_font(cache: &Cache, f: impl FnOnce(PatternRef<'_>)) {
    let font = cache.fonts().expect("fonts").next().expect("at least one font");
    f(font);
}

#[test]
fn a_config_without_selectfont_has_no_selectors() {
    // Named search path, not the environment's. That fixture has a bare
    // `<include>conf.d</include>`, and a relative include resolves against
    // the search path -- `FONTCONFIG_PATH` and then the built-in
    // configuration directory -- so loading it with the default path pulls in
    // the *host's* `/etc/fonts/conf.d`, whose `<selectfont>` rules are
    // exactly what this is asserting the absence of. It passes on a machine
    // with no `/etc/fonts` and fails on one with it.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config");
    let plain =
        Config::load_from_with_path(&dir.join("fonts.conf"), std::slice::from_ref(&dir)).unwrap();
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
fn a_selector_naming_a_property_we_cannot_resolve_never_matches() {
    // A property fontconfig assigns at runtime cannot be resolved here, so
    // the selector must not narrow to the elements around it: dropping the
    // element would *widen* it, and a reject rule that widens rejects fonts
    // fontconfig keeps. Measured: with this config `fc-list` keeps the font.
    let cache = cantarell();
    let config = fixture("unknown-object.conf");
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(kept, 6, "the selector narrowed to its understood half");
}

/// A `<langset>` with no usable names contributes *nothing*, and a `<patelt>`
/// left with nothing is not in the pattern at all.
///
/// `FcParseLangSet` pushes its set only when it took at least one name, so
/// `<langset>en</langset>` -- bare text where `<string>en</string>` was meant
/// -- is `FcTypeVoid`, `FcParsePatelt` stops at it, and the selector is
/// whatever elements came before. Here that is the family alone, which
/// matches. Measured: `fc-list` rejects the font for this config, where this
/// crate used to treat the whole selector as unevaluable and keep it.
#[test]
fn a_value_element_with_nothing_usable_in_it_leaves_the_patelt_out() {
    let cache = cantarell();
    for name in ["langset-poison.conf", "charset-empty.conf"] {
        let config = fixture(name);
        let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
        assert_eq!(kept, 0, "{name}: the family selector alone still rejects");
    }
}

/// A `<const>` naming nothing is *not* one of those. Fontconfig resolves it
/// -- to `FcTypeVoid` -- and `FcParsePatelt` stops at the first Void value,
/// so the `<patelt>` adds nothing to the pattern and the elements beside it
/// still apply. This fixture pairs the unknown constant with a family that
/// matches, and the family alone is enough to reject.
///
/// Checked against `fc-list`, which keeps exactly as many fonts for
/// `family + unknown const` as for `family` alone.
#[test]
fn an_unknown_const_drops_its_element_rather_than_the_selector() {
    let config = fixture("const-unknown.conf");
    let cache = cantarell();
    let kept = cache.fonts().unwrap().filter(|f| config.accepts(f)).count();
    assert_eq!(kept, 0, "the family element should still have rejected every instance");
}

/// Selector strings are compared with full Unicode case folding, not ASCII
/// lowercasing. Nothing on a typical system exercises this, so it is checked
/// directly rather than through a font.
#[test]
fn selector_strings_fold_beyond_ascii() {
    use typordo::casefold;
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
