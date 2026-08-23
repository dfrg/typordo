//! Tests for `<match>` and `<alias>` substitution.

use std::path::Path;

use fontconf::{Binding, Config, MatchKind, Object, OwnedValue, Query};

fn config() -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules/fonts.conf");
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The family list after substitution, with each value's binding.
fn families(config: &Config, query: &mut Query) -> Vec<(String, Binding)> {
    config.substitute(query);
    query
        .get(Object::Family)
        .map(|element| {
            element
                .values()
                .filter_map(|(value, binding)| match value {
                    OwnedValue::String(s) => Some((s.clone(), binding)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn with_family(name: &str) -> Query {
    let mut query = Query::new();
    query.add(Object::Family, name);
    query
}

/// `<prefer>` goes in front of the matched family and `<default>` after it,
/// which is the difference between overriding a request and backstopping it.
#[test]
fn an_alias_prefers_before_and_defaults_after() {
    let config = config();
    let names: Vec<String> = families(&config, &mut with_family("sans-serif"))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["Preferred Sans", "Second Sans", "sans-serif", "Last Resort Sans"]);
}

/// `binding="same"` inherits from the value a test marked -- but only where
/// the edit inserts relative to that mark. `append_last` ignores the mark, so
/// its values stay weak even though the matched family was strong.
#[test]
fn binding_same_inherits_only_where_a_position_is_used() {
    let config = config();
    let bindings = families(&config, &mut with_family("Inheriting"));
    let find = |name: &str| {
        bindings
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} missing from {bindings:?}"))
            .1
    };
    // The caller's own family is strong.
    assert_eq!(find("Inheriting"), Binding::Strong);
    // <accept> appends relative to the mark, so it inherits strong.
    assert_eq!(find("Accepted Same"), Binding::Strong);
    // <default> appends last, which passes no position, so it stays weak.
    assert_eq!(find("Defaulted Same"), Binding::Weak);
}

#[test]
fn a_test_and_assign_replaces_the_matched_value() {
    let config = config();
    let names: Vec<String> = families(&config, &mut with_family("Rename Me"))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["Renamed"]);
}

#[test]
fn expressions_read_the_pattern_and_do_arithmetic() {
    let config = config();
    let mut query = with_family("Doubler");
    query.add(Object::Weight, 100);
    config.substitute(&mut query);
    assert_eq!(query.number(Object::Weight), Some(200.0));
}

#[test]
fn a_conditional_picks_a_branch() {
    let config = config();
    let mut heavy = with_family("Conditional");
    heavy.add(Object::Weight, 200);
    config.substitute(&mut heavy);
    assert_eq!(heavy.number(Object::Slant), Some(100.0));

    let mut light = with_family("Conditional");
    light.add(Object::Weight, 50);
    config.substitute(&mut light);
    assert_eq!(light.number(Object::Slant), Some(0.0));
}

/// Every test in a `<match>` must pass. The second one here cannot, so the
/// edit after it must not run.
#[test]
fn one_failing_test_abandons_the_rule() {
    let config = config();
    let mut query = with_family("NeverMatches");
    query.add(Object::Weight, 80);
    let names: Vec<String> =
        families(&config, &mut query).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["NeverMatches"]);
}

#[test]
fn delete_all_and_prepend_first() {
    let config = config();
    let mut query = with_family("Deleter");
    query.add(Object::Spacing, 100);
    config.substitute(&mut query);
    assert!(!query.contains(Object::Spacing), "spacing should be gone");
    let names: Vec<String> = query
        .get(Object::Family)
        .unwrap()
        .values()
        .filter_map(|(v, _)| match v {
            OwnedValue::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["Prepended", "Deleter"]);
}

/// Substitution runs before the defaults, so a rule that sets weight is not
/// then overwritten by the default weight.
#[test]
fn substitution_runs_before_default_substitute() {
    let config = config();
    let mut query = with_family("Doubler");
    query.add(Object::Weight, 100);
    config.substitute(&mut query);
    query.default_substitute();
    assert_eq!(query.number(Object::Weight), Some(200.0));
}

/// A query nothing matches keeps its own values, but substitution is not a
/// no-op even then: it always injects the locale's languages first.
#[test]
fn an_unmatched_query_keeps_its_values() {
    let config = config();
    let mut query = with_family("Untouched");
    config.substitute(&mut query);
    assert_eq!(query.string(Object::Family), Some("Untouched"));
    assert_eq!(query.get(Object::Family).unwrap().values().count(), 1);
}

/// `FcConfigSubstitute` adds the locale's languages before any rule runs, and
/// weakly, so a font that answers them is preferred without outranking the
/// caller's own choices. Sorting demotes fonts that answer no requested
/// language, so leaving this out reorders an entire fallback chain.
#[test]
fn substitution_injects_the_locale_languages() {
    let config = config();
    let mut query = with_family("Untouched");
    config.substitute(&mut query);

    let langs = query.get(Object::Lang).expect("a language was added");
    let values: Vec<_> = langs.values().collect();
    assert!(!values.is_empty());
    for (value, binding) in &values {
        assert!(matches!(value, OwnedValue::String(_)), "{value:?}");
        assert_eq!(*binding, Binding::Weak, "injected languages must be weak");
    }
    assert_eq!(
        values
            .iter()
            .filter_map(|(v, _)| match v {
                OwnedValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        fontconf::default_langs().iter().map(String::as_str).collect::<Vec<_>>()
    );
}

/// The injection stops only on an exact match or on `und`, not on any
/// language at all: asking for Japanese still gets the locale's language
/// appended, because a font good at both is a better answer than one good at
/// only Japanese.
#[test]
fn injection_stops_at_und_but_not_at_an_unrelated_language() {
    let langs_after = |existing: &str| -> Vec<String> {
        let config = config();
        let mut query = with_family("Untouched");
        query.add(Object::Lang, existing);
        config.substitute(&mut query);
        query
            .get(Object::Lang)
            .unwrap()
            .values()
            .filter_map(|(v, _)| match v {
                OwnedValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    };

    // "und" means "do not assume", so nothing is added.
    assert_eq!(langs_after("und"), ["und"]);
    // An unrelated language does not stop it.
    let with_ja = langs_after("ja");
    assert_eq!(with_ja[0], "ja");
    assert!(with_ja.len() > 1, "the locale language should still be appended");
    // The locale's own language is already there, so it is not duplicated.
    let locale = fontconf::default_langs().remove(0);
    assert_eq!(langs_after(&locale), [locale]);
}

// --- custom properties and name targets -----------------------------------

/// A config can assign to a name fontconfig does not define, and read it back
/// in a later rule. `10-scale-bitmap-fonts.conf` computes
/// `pixelsizefixupfactor` this way, so dropping unknown names breaks it.
#[test]
fn a_config_can_invent_a_property_and_read_it_back() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules/custom.conf");
    let config = Config::load_from(&path).unwrap();

    let mut query = Query::new();
    query.add(Object::Family, "Scratch");
    query.add(Object::PixelSize, 24.0);
    config.substitute(&mut query);

    // The first rule stored a factor, the second read it back into weight.
    let custom = query.custom("scratchfactor").expect("custom property kept");
    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0].0, OwnedValue::Double(3.0));
    assert_eq!(query.number(Object::Weight), Some(72.0));
}

/// The languages a font is left with after the scan rules run.
fn scanned_langs(config: &Config, family: &str, langs: &[&str]) -> Vec<String> {
    let mut set = fontconf::OwnedLangSet::new();
    for lang in langs {
        set.insert_index(fontconf::langs::index_of(lang).expect(lang));
    }
    let mut font = Query::new();
    font.add(Object::Family, family);
    font.add(Object::Lang, OwnedValue::LangSet(set));
    config.substitute_kind(&mut font, MatchKind::Scan, None);
    match font.value(Object::Lang) {
        Some(OwnedValue::LangSet(set)) => set.langs().map(str::to_string).collect(),
        other => panic!("lang became {other:?}"),
    }
}

/// A `target="scan"` rule can subtract a language set, which is how a config
/// takes Devanagari away from a font that covers the codepoints and renders
/// none of the conjuncts.
#[test]
fn a_scan_rule_subtracts_languages() {
    let config = config();
    let left = scanned_langs(&config, "Overclaims", &["en", "hi", "mr", "ja"]);
    assert_eq!(left, ["en", "ja"]);
}

/// A name fontconfig does not know goes into a set this crate cannot hold.
/// Dropping it must not drop the whole edit: the rest of the subtraction
/// still has to happen.
#[test]
fn an_unknown_language_does_not_void_the_edit() {
    let config = config();
    let left = scanned_langs(&config, "Overclaims", &["hi"]);
    assert!(left.is_empty(), "expected nothing left, got {left:?}");
}

#[test]
fn a_scan_rule_unions_languages() {
    let config = config();
    let left = scanned_langs(&config, "Underclaims", &["en"]);
    assert_eq!(left, ["en", "ja"]);
}

/// Scan rules are a different target: nothing should happen to a pattern
/// being matched.
#[test]
fn scan_rules_do_not_run_at_match_time() {
    let config = config();
    let mut set = fontconf::OwnedLangSet::new();
    set.insert_index(fontconf::langs::index_of("hi").unwrap());
    let mut query = Query::new();
    query.add(Object::Family, "Overclaims");
    query.add(Object::Lang, OwnedValue::LangSet(set));
    config.substitute(&mut query);
    match query.value(Object::Lang) {
        Some(OwnedValue::LangSet(set)) => assert!(set.langs().any(|l| l == "hi")),
        other => panic!("lang became {other:?}"),
    }
}

/// A `<range>` literal, which the expression parser had no value shape for
/// until ranges existed.
#[test]
fn a_range_literal_is_assigned() {
    let config = config();
    let mut query = with_family("Ranged");
    config.substitute(&mut query);
    match query.value(Object::Weight) {
        Some(OwnedValue::Range(range)) => {
            assert_eq!((range.begin, range.end), (40.0, 210.0));
        }
        other => panic!("weight became {other:?}"),
    }
}

/// A `<charset>` literal, whose children used to be collected into a list the
/// expression path never read -- so it always came out empty.
#[test]
fn a_charset_literal_expands_its_spans() {
    let config = config();
    let mut query = with_family("Spanned");
    config.substitute(&mut query);
    match query.value(Object::Charset) {
        Some(OwnedValue::CharSet(set)) => {
            assert_eq!(set.chars().collect::<String>(), "Aabc");
        }
        other => panic!("charset became {other:?}"),
    }
}

/// The same bug, and the reason to look for it: a `<matrix>` in a rule read
/// `exprs` while its children had gone into `values`.
#[test]
fn a_matrix_literal_is_assigned() {
    let config = config();
    let mut query = with_family("Skewed");
    config.substitute(&mut query);
    match query.value(Object::Matrix) {
        Some(OwnedValue::Matrix(m)) => {
            assert_eq!((m.xx, m.xy, m.yx, m.yy), (1.0, 0.2, 0.0, 1.0));
        }
        other => panic!("matrix became {other:?}"),
    }
}
