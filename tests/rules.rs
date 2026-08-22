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

#[test]
fn the_fixture_parses_into_rules() {
    let config = config();
    assert_eq!(config.rules().len(), 7, "{:?}", config.rules().len());
    assert!(config.rules().iter().all(|r| r.kind == MatchKind::Pattern));
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
    assert_eq!(
        names,
        ["Preferred Sans", "Second Sans", "sans-serif", "Last Resort Sans"]
    );
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
    let names: Vec<String> = families(&config, &mut query)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
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

/// A query nothing matches passes through untouched.
#[test]
fn an_unmatched_query_is_left_alone() {
    let config = config();
    let mut query = with_family("Untouched");
    let before = query.clone();
    config.substitute(&mut query);
    assert_eq!(query, before);
}
