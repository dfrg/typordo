//! Tests for scanning font files, against the checked-in fixture.
//!
//! Only the Cantarell fixture is a font we have; the cache fixtures record
//! what fontconfig made of it, so the cache doubles as the expected answer.

use std::path::{Path, PathBuf};

use typordo::{Cache, Object, Pattern, Tristate, Value};

fn fixture_font() -> Option<PathBuf> {
    // The font itself is not vendored -- only the cache fontconfig built from
    // it -- so these tests run only where the font is installed.
    let path = PathBuf::from("/usr/share/fonts/abattis-cantarell-vf-fonts/Cantarell-VF.otf");
    path.exists().then_some(path)
}

fn cached() -> Cache {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

fn string(pattern: &Pattern, object: Object) -> Option<String> {
    match pattern.value(object)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A variable font is not one face. Fontconfig records the default instance,
/// then each named instance that is not the default, then one pattern for the
/// variable font itself.
#[test]
fn a_variable_font_scans_to_the_faces_the_cache_recorded() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");
    let cache = cached();
    let recorded: Vec<_> = cache.fonts().unwrap().collect();

    assert_eq!(
        scanned.len(),
        recorded.len(),
        "scanned {} faces, cache has {}",
        scanned.len(),
        recorded.len()
    );

    for (ours, theirs) in scanned.iter().zip(&recorded) {
        assert_eq!(string(ours, Object::Style).as_deref(), theirs.string(Object::Style), "style");
        let ours_index = match ours.value(Object::Index) {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        };
        assert_eq!(
            ours_index,
            theirs.int(Object::Index),
            "index for style {:?}",
            theirs.string(Object::Style)
        );
    }
}

/// The index encodes which instance a pattern is: the ordinal in the high
/// half, the face in the low half, and zero for the default.
#[test]
fn the_index_identifies_the_named_instance() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");

    let indices: Vec<i32> = scanned
        .iter()
        .filter_map(|p| match p.value(Object::Index)? {
            Value::Int(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(indices[0], 0, "the default instance is index zero");
    // Every named instance keeps face 0 in the low half.
    for index in &indices {
        assert_eq!(index & 0xffff, 0, "face index should be zero: {index:#x}");
    }
    // The last pattern is the variable one, which is also index zero.
    assert_eq!(*indices.last().unwrap(), 0);
}

#[test]
fn the_variable_pattern_carries_ranges_and_the_others_do_not() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");

    let variable = scanned
        .iter()
        .find(|p| p.value(Object::Variable) == Some(&Value::Bool(Tristate::True)))
        .expect("one variable pattern");
    assert!(
        matches!(variable.value(Object::Weight), Some(Value::Range(_))),
        "the variable pattern's weight should be a range"
    );
    // A named instance pins a value instead.
    let instance = scanned
        .iter()
        .find(|p| p.value(Object::NamedInstance) == Some(&Value::Bool(Tristate::True)))
        .expect("a named instance");
    assert!(matches!(instance.value(Object::Weight), Some(Value::Double(_))));
}

#[test]
fn unrecognized_bytes_are_rejected_rather_than_guessed_at() {
    assert!(typordo::scan_bytes(b"not a font at all", "x").is_err());
    assert!(typordo::scan_bytes(&[], "x").is_err());
}

/// A `.otb` bitmap font carries an empty `glyf`, which is not outlines.
#[test]
fn an_empty_glyf_table_is_not_an_outline_font() {
    let path = PathBuf::from("/usr/share/fonts/terminus-fonts/ter-u12n.otb");
    if !path.exists() {
        return;
    }
    let scanned = typordo::scan_file(&path).expect("scan");
    let font = &scanned[0];
    assert_eq!(font.value(Object::Outline), Some(&Value::Bool(Tristate::False)));
    assert_eq!(font.value(Object::Scalable), Some(&Value::Bool(Tristate::False)));
}

// --- coverage -------------------------------------------------------------

/// The languages a font supports are derived from what it covers, not
/// declared: fontconfig asks whether every codepoint of each language's
/// orthography is present.
#[test]
fn languages_are_derived_from_the_characters_covered() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");
    let cache = cached();
    let recorded = cache.fonts().unwrap().next().unwrap();

    let ours = match scanned[0].value(Object::Lang) {
        Some(Value::LangSet(langs)) => typordo::AnyLangSet::Owned(langs),
        other => panic!("expected a langset, got {other:?}"),
    };
    let theirs = match recorded.value(Object::Lang) {
        Some(typordo::ValueRef::LangSet(langs)) => langs,
        other => panic!("expected a cached langset, got {other:?}"),
    };
    assert_eq!(
        ours.langs().collect::<Vec<_>>(),
        theirs.langs().collect::<Vec<_>>(),
        "derived languages should match what fontconfig cached"
    );
}

#[test]
fn coverage_matches_what_fontconfig_cached() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");
    let cache = cached();
    let recorded = cache.fonts().unwrap().next().unwrap();

    let ours = match scanned[0].value(Object::Charset) {
        Some(Value::CharSet(coverage)) => typordo::AnyCharSet::Owned(coverage),
        other => panic!("expected a charset, got {other:?}"),
    };
    let theirs = match recorded.value(Object::Charset) {
        Some(typordo::ValueRef::CharSet(chars)) => chars,
        other => panic!("expected a cached charset, got {other:?}"),
    };
    assert_eq!(ours.len(), theirs.len(), "coverage size");
    assert_eq!(ours.to_string(), theirs.to_string(), "coverage ranges");
}

/// A NUL mapped to the blank glyph is not coverage. Fonts routinely map the
/// ASCII control range to `.notdef` or to the space glyph, and counting
/// either puts characters in a font's charset that it cannot draw.
#[test]
fn control_characters_are_not_counted_unless_they_draw() {
    let Some(path) = fixture_font() else { return };
    let scanned = typordo::scan_file(&path).expect("scan");
    let coverage = match scanned[0].value(Object::Charset) {
        Some(Value::CharSet(c)) => c,
        other => panic!("expected a charset, got {other:?}"),
    };
    assert!(!coverage.contains('\u{0}'), "NUL should not be covered");
    assert!(!coverage.contains('\r'), "carriage return should not be covered");
    assert!(coverage.contains('A'));
}

/// A Type 1 font has no cmap at all: its coverage comes from glyph names
/// mapped through the Adobe Glyph List.
#[test]
fn a_type1_font_reports_coverage_from_its_glyph_names() {
    let path = PathBuf::from("/usr/share/fonts/urw-base35/NimbusRoman-Regular.t1");
    if !path.exists() {
        return;
    }
    let scanned = typordo::scan_file(&path).expect("scan");
    let coverage = match scanned[0].value(Object::Charset) {
        Some(Value::CharSet(c)) => c,
        other => panic!("expected a charset, got {other:?}"),
    };
    for c in ['A', 'z', '0', '!'] {
        assert!(coverage.contains(c), "{c:?} missing from a Latin Type 1 font");
    }
    assert!(!coverage.contains('\u{4e00}'));

    // And that coverage is enough to name languages.
    let langs = match scanned[0].value(Object::Lang) {
        Some(Value::LangSet(l)) => l,
        other => panic!("expected a langset, got {other:?}"),
    };
    assert!(langs.langs().any(|l| l == "en"), "should support English");
}

/// Localized names are labelled with the language they are written in, and
/// the English one comes first. A font that lists its style in twenty
/// languages has to report them in fontconfig's order or `%{style}` reads
/// differently.
#[test]
fn localized_names_are_tagged_and_english_leads() {
    let path = PathBuf::from("/usr/share/fonts/gnu-free/FreeMonoBold.ttf");
    if !path.exists() {
        return;
    }
    let scanned = typordo::scan_file(&path).expect("scan");
    let font = &scanned[0];

    let styles: Vec<String> = font
        .get(Object::Style)
        .unwrap()
        .values()
        .filter_map(|(v, _)| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(styles.len() > 5, "expected many localizations: {styles:?}");
    assert_eq!(styles[0], "Bold", "the English name comes first");

    // Every name is paired with a language.
    let langs = font.get(Object::Stylelang).unwrap().values().count();
    assert_eq!(langs, styles.len(), "each name needs its language");

    // The same word in two cases is one name, not two.
    let mut folded: Vec<String> =
        styles.iter().map(|s| typordo::casefold::fold_str(s).collect()).collect();
    folded.sort();
    let before = folded.len();
    folded.dedup();
    assert_eq!(folded.len(), before, "names should already be deduplicated");
}
