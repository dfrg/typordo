//! Tests for scanning font files, against the checked-in fixture.
//!
//! Only the Cantarell fixture is a font we have; the cache fixtures record
//! what fontconfig made of it, so the cache doubles as the expected answer.

use std::path::{Path, PathBuf};

use fontconf::{Cache, Object, OwnedValue, Query};

fn fixture_font() -> Option<PathBuf> {
    // The font itself is not vendored -- only the cache fontconfig built from
    // it -- so these tests run only where the font is installed.
    let path = PathBuf::from("/usr/share/fonts/abattis-cantarell-vf-fonts/Cantarell-VF.otf");
    path.exists().then_some(path)
}

fn cached() -> Cache {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

fn string(pattern: &Query, object: Object) -> Option<String> {
    match pattern.value(object)? {
        OwnedValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A variable font is not one face. Fontconfig records the default instance,
/// then each named instance that is not the default, then one pattern for the
/// variable font itself.
#[test]
fn a_variable_font_scans_to_the_faces_the_cache_recorded() {
    let Some(path) = fixture_font() else { return };
    let scanned = fontconf::scan_file(&path).expect("scan");
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
        assert_eq!(
            string(ours, Object::Style).as_deref(),
            theirs.string(Object::Style),
            "style"
        );
        let ours_index = match ours.value(Object::Index) {
            Some(OwnedValue::Int(i)) => Some(*i),
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
    let scanned = fontconf::scan_file(&path).expect("scan");

    let indices: Vec<i32> = scanned
        .iter()
        .filter_map(|p| match p.value(Object::Index)? {
            OwnedValue::Int(i) => Some(*i),
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
    let scanned = fontconf::scan_file(&path).expect("scan");

    let variable = scanned
        .iter()
        .find(|p| p.value(Object::Variable) == Some(&OwnedValue::Bool(true)))
        .expect("one variable pattern");
    assert!(
        matches!(variable.value(Object::Weight), Some(OwnedValue::Range(_))),
        "the variable pattern's weight should be a range"
    );
    // A named instance pins a value instead.
    let instance = scanned
        .iter()
        .find(|p| p.value(Object::NamedInstance) == Some(&OwnedValue::Bool(true)))
        .expect("a named instance");
    assert!(matches!(instance.value(Object::Weight), Some(OwnedValue::Double(_))));
}

#[test]
fn unrecognized_bytes_are_rejected_rather_than_guessed_at() {
    assert!(fontconf::scan_bytes(b"not a font at all", "x").is_err());
    assert!(fontconf::scan_bytes(&[], "x").is_err());
}

/// A `.otb` bitmap font carries an empty `glyf`, which is not outlines.
#[test]
fn an_empty_glyf_table_is_not_an_outline_font() {
    let path = PathBuf::from("/usr/share/fonts/terminus-fonts/ter-u12n.otb");
    if !path.exists() {
        return;
    }
    let scanned = fontconf::scan_file(&path).expect("scan");
    let font = &scanned[0];
    assert_eq!(font.value(Object::Outline), Some(&OwnedValue::Bool(false)));
    assert_eq!(font.value(Object::Scalable), Some(&OwnedValue::Bool(false)));
}
