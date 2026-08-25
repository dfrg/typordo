//! Tests for `<match>` and `<alias>` substitution.

use std::path::Path;

use typordo::{Binding, Config, MatchKind, Object, Pattern, Value};

fn config() -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules/fonts.conf");
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The family list after substitution, with each value's binding.
fn families(config: &Config, query: &mut Pattern) -> Vec<(String, Binding)> {
    config.substitute(query);
    query
        .get(Object::Family)
        .map(|element| {
            element
                .values()
                .filter_map(|(value, binding)| match value {
                    Value::String(s) => Some((s.clone(), binding)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn with_family(name: &str) -> Pattern {
    let mut query = Pattern::new();
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
            Value::String(s) => Some(s.clone()),
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
        assert!(matches!(value, Value::String(_)), "{value:?}");
        assert_eq!(*binding, Binding::Weak, "injected languages must be weak");
    }
    assert_eq!(
        values
            .iter()
            .filter_map(|(v, _)| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        typordo::default_langs().iter().map(String::as_str).collect::<Vec<_>>()
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
                Value::String(s) => Some(s.clone()),
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
    let locale = typordo::default_langs().remove(0);
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

    let mut query = Pattern::new();
    query.add(Object::Family, "Scratch");
    query.add(Object::PixelSize, 24.0);
    config.substitute(&mut query);

    // The first rule stored a factor, the second read it back into weight.
    let custom = query.custom("scratchfactor").expect("custom property kept");
    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0].0, Value::Double(3.0));
    assert_eq!(query.number(Object::Weight), Some(72.0));
}

/// The languages a font is left with after the scan rules run.
fn scanned_langs(config: &Config, family: &str, langs: &[&str]) -> Vec<String> {
    let mut set = typordo::LangSet::new();
    for lang in langs {
        set.insert_index(typordo::langs::index_of(lang).expect(lang));
    }
    let mut font = Pattern::new();
    font.add(Object::Family, family);
    font.add(Object::Lang, Value::LangSet(set));
    config.substitute_kind(&mut font, MatchKind::Scan, None);
    match font.value(Object::Lang) {
        Some(Value::LangSet(set)) => set.langs().map(str::to_string).collect(),
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
    let mut set = typordo::LangSet::new();
    set.insert_index(typordo::langs::index_of("hi").unwrap());
    let mut query = Pattern::new();
    query.add(Object::Family, "Overclaims");
    query.add(Object::Lang, Value::LangSet(set));
    config.substitute(&mut query);
    match query.value(Object::Lang) {
        Some(Value::LangSet(set)) => assert!(set.langs().any(|l| l == "hi")),
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
        Some(Value::Range(range)) => {
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
        Some(Value::CharSet(set)) => {
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
        Some(Value::Matrix(m)) => {
            assert_eq!((m.xx, m.xy, m.yx, m.yy), (1.0, 0.2, 0.0, 1.0));
        }
        other => panic!("matrix became {other:?}"),
    }
}

// --- ignore-blanks --------------------------------------------------------

fn blanks_config() -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules/blanks.conf");
    Config::load_from(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A plain `<test>` folds case but not blanks.
///
/// `FcConfigCompareValue` reaches for `FcStrCmpIgnoreBlanksAndCase` only when
/// `FcOpFlagIgnoreBlanks` is set, and `FcStrCmpIgnoreCase` otherwise. This
/// crate used to strip blanks for every string equality, which made a
/// `<test>` fire on names fontconfig considers different.
#[test]
fn a_plain_test_treats_a_space_as_a_character() {
    let config = blanks_config();

    // Exactly the name the test asks for, spaces included.
    let mut exact = Pattern::new();
    exact.add(Object::Family, "DejaVu Sans");
    config.substitute(&mut exact);
    assert_eq!(exact.string(Object::Foundry), Some("blanks-significant"));

    // The same name without the space is a different name.
    let mut squashed = Pattern::new();
    squashed.add(Object::Family, "DejaVuSans");
    config.substitute(&mut squashed);
    assert_eq!(squashed.string(Object::Foundry), None, "a blank must not be ignored here");

    // Case still folds, blanks and all.
    let mut shouted = Pattern::new();
    shouted.add(Object::Family, "DEJAVU SANS");
    config.substitute(&mut shouted);
    assert_eq!(shouted.string(Object::Foundry), Some("blanks-significant"));
}

/// `ignore-blanks="true"` is what makes the two spellings the same name.
#[test]
fn ignore_blanks_makes_a_space_invisible() {
    let config = blanks_config();
    for family in ["DejaVuSans", "DejaVu Sans", "Deja Vu Sans", "dejavusans"] {
        let mut query = Pattern::new();
        query.add(Object::Family, family);
        config.substitute(&mut query);
        assert_eq!(query.number(Object::Weight), Some(210.0), "{family}");
    }
}

/// An `<alias>` may carry `<test>` elements, which make it conditional --
/// `FcParseAlias` places them ahead of the family test it synthesizes, and
/// every one has to pass. Discarding them turns a conditional alias into an
/// unconditional one, which is worse than ignoring it: it fires where the
/// author said it must not.
#[test]
fn an_alias_applies_only_when_its_own_tests_pass() {
    let config = config();

    let mut heavy = with_family("Conditional");
    heavy.add(Object::Weight, 210);
    let names: Vec<String> =
        families(&config, &mut heavy).into_iter().map(|(name, _)| name).collect();
    assert!(names.contains(&"Heavy Substitute".to_string()), "{names:?}");

    let mut light = with_family("Conditional");
    light.add(Object::Weight, 80);
    let names: Vec<String> =
        families(&config, &mut light).into_iter().map(|(name, _)| name).collect();
    assert!(!names.contains(&"Heavy Substitute".to_string()), "{names:?}");
}

/// `FcDefaultSubstitute` ends by adding `prgname`, `desktop` and `order`, and
/// `FcConfigSubstituteWithPat` adds `prgname` again before any pattern rule
/// runs. None of the three is scored against: they are there so a
/// configuration can test them, and a `<test name="prgname">` rule cannot
/// fire against a property nothing ever sets.
#[test]
fn substitution_supplies_the_properties_a_config_may_test() {
    let mut query = with_family("serif");
    query.default_substitute();

    let prgname = query.get(Object::Prgname).and_then(|e| {
        e.values().find_map(|(v, _)| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    });
    // The test binary's own name, whatever cargo called it.
    assert!(prgname.is_some_and(|n| !n.is_empty()), "prgname should name this executable");

    let order = query.get(Object::Order).and_then(|e| {
        e.values().find_map(|(v, _)| match v {
            Value::Int(i) => Some(*i),
            _ => None,
        })
    });
    assert_eq!(order, Some(0));

    // `desktop` follows XDG_CURRENT_DESKTOP and is absent when that is unset
    // or empty, so its presence is not asserted -- only that a value already
    // in the pattern is left alone, which is the rule for all three.
    let mut kept = with_family("serif");
    kept.add(Object::Order, 42);
    kept.add(Object::Prgname, "chosen");
    kept.default_substitute();
    let orders: Vec<i32> = kept
        .get(Object::Order)
        .map(|e| {
            e.values()
                .filter_map(|(v, _)| match v {
                    Value::Int(i) => Some(*i),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(orders, [42], "a value already present is not joined by a default");
}

/// A pattern rule testing `prgname` has to see it, which means substitution
/// supplies it before the rules run rather than leaving it to the defaults.
#[test]
fn a_pattern_rule_can_test_prgname() {
    let config = config();
    let mut query = with_family("serif");
    config.substitute(&mut query);
    assert!(query.contains(Object::Prgname), "prgname must be set before pattern rules run");
}

/// The locale's languages are appended only when the query is not already
/// asking for one of them -- and the query may say so either way.
///
/// `FcNameParse` builds a *langset* for `:lang=en`, because `lang` is
/// declared as one, while a caller assembling a pattern by hand usually adds
/// a string. Fontconfig checks both shapes. Checking only strings meant a
/// langset query never counted as asking for its own language, so the
/// locale's languages were appended beside it -- and the extra weak value was
/// enough to move a variable font forty places in `fc-match -a`.
#[test]
fn the_locale_languages_see_a_langset_query() {
    use typordo::LangSet;

    let with = |value: Value| {
        let mut query = with_family("serif");
        match value {
            Value::LangSet(set) => query.add(Object::Lang, set),
            Value::String(s) => query.add(Object::Lang, s.as_str()),
            _ => unreachable!(),
        };
        let config = config();
        config.substitute(&mut query);
        query.get(Object::Lang).map(|e| e.values().count()).expect("the query asked for a language")
    };

    let mut set = LangSet::new();
    set.insert("en");
    let langset_values = with(Value::LangSet(set));
    let string_values = with(Value::String("en".into()));
    assert_eq!(
        langset_values, string_values,
        "a langset asking for en must stop the injection exactly as a string does"
    );
    assert_eq!(langset_values, 1, "nothing should have been appended");
}
