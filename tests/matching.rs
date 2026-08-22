//! Scoring tests against the checked-in cache.
//!
//! The fixture is Cantarell VF: one variable pattern spanning weight 0..205,
//! plus five named instances (Regular 80, Thin 0, Light 50, Bold 200,
//! Extra Bold 205). That is enough to exercise family matching, ranges,
//! priority order and tie-breaking without a font system present.

use fontconf::{Cache, Object, Priority, Query, Score};

fn cantarell() -> Cache {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cantarell-le64.cache-9");
    Cache::open(&path).expect("fixture cache")
}

/// The style of the font a query picks.
fn best_style(cache: &Cache, query: &Query) -> Option<String> {
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let (font, _) = fontconf::best(query, fonts)?;
    Some(font.string(Object::Style).unwrap_or("<none>").to_string())
}

fn query(build: impl FnOnce(&mut Query)) -> Query {
    let mut q = Query::new();
    build(&mut q);
    q.default_substitute();
    q
}

#[test]
fn default_substitute_fills_what_matching_needs() {
    let q = query(|q| {
        q.add(Object::Family, "Cantarell");
    });
    assert_eq!(q.number(Object::Weight), Some(80.0)); // normal
    assert_eq!(q.number(Object::Slant), Some(0.0)); // roman
    assert_eq!(q.number(Object::Width), Some(100.0)); // normal
    assert_eq!(q.number(Object::Size), Some(12.0));
    // pixelsize = size * scale * dpi / 72
    assert_eq!(q.number(Object::PixelSize), Some(12.0 * 1.0 * 75.0 / 72.0));
    assert_eq!(q.number(Object::Fontversion), Some(f64::from(0x7fff_ffff)));

    // An explicit value is never overwritten.
    let q = query(|q| {
        q.add(Object::Weight, 200);
        q.add(Object::PixelSize, 24.0);
    });
    assert_eq!(q.number(Object::Weight), Some(200.0));
    assert_eq!(q.number(Object::PixelSize), Some(24.0));
    // ...and size is derived back from pixelsize rather than defaulted to 12.
    assert_eq!(q.number(Object::Size), Some(24.0 / 75.0 * 72.0));
}

#[test]
fn a_weight_request_picks_the_matching_instance() {
    let cache = cantarell();
    for (weight, expected) in [
        (80, "Regular"),
        (0, "Thin"),
        (50, "Light"),
        (200, "Bold"),
        (205, "Extra Bold"),
    ] {
        let q = query(|q| {
            q.add(Object::Family, "Cantarell");
            q.add(Object::Weight, weight);
        });
        assert_eq!(best_style(&cache, &q).as_deref(), Some(expected), "weight={weight}");
    }
}

/// A weight nothing has exactly goes to the nearest, by absolute distance.
#[test]
fn an_unavailable_weight_picks_the_nearest() {
    let cache = cantarell();
    // 190 is 10 from Bold (200) and 110 from Regular (80).
    let q = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Weight, 190);
    });
    assert_eq!(best_style(&cache, &q).as_deref(), Some("Bold"));
}

/// Families match ignoring case and blanks, and nothing else.
#[test]
fn family_matching_ignores_case_and_blanks() {
    let cache = cantarell();
    for name in ["Cantarell", "cantarell", "CANTARELL", "  Cant arell "] {
        let q = query(|q| {
            q.add(Object::Family, name);
        });
        let fonts: Vec<_> = cache.fonts().unwrap().collect();
        let (font, score) = fontconf::best(&q, fonts).expect("a match");
        assert_eq!(font.string(Object::Family), Some("Cantarell"));
        assert_eq!(score.get(Priority::FamilyStrong), 0.0, "{name} should match exactly");
    }
}

/// A family no font has scores as no match at all, but still returns
/// something: fontconfig always answers, it just answers badly.
#[test]
fn an_unknown_family_still_returns_a_font() {
    let cache = cantarell();
    let q = query(|q| {
        q.add(Object::Family, "No Such Family");
    });
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let (_, score) = fontconf::best(&q, fonts).expect("fontconfig always answers");
    assert!(score.get(Priority::FamilyStrong) > 1e90, "should be the no-match sentinel");
}

/// The earlier a family is listed, the better it scores, so a first-choice
/// family beats a second-choice one regardless of anything after it.
#[test]
fn family_order_is_the_score() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();

    let first = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Family, "Other");
    });
    let second = query(|q| {
        q.add(Object::Family, "Other");
        q.add(Object::Family, "Cantarell");
    });
    let (_, a) = fontconf::best(&first, fonts.clone()).unwrap();
    let (_, b) = fontconf::best(&second, fonts).unwrap();
    assert_eq!(a.get(Priority::FamilyStrong), 0.0);
    assert_eq!(b.get(Priority::FamilyStrong), 1.0);
    assert!(a.beats(&b));
}

/// A weakly bound family scores in the weak slot, which sits *below* language
/// in the priority order while the strong slot sits above it.
#[test]
fn binding_decides_which_family_slot_is_used() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();

    let mut weak = Query::new();
    weak.add_weak(Object::Family, "Cantarell");
    weak.default_substitute();

    let (_, score) = fontconf::best(&weak, fonts).unwrap();
    assert_eq!(score.get(Priority::FamilyWeak), 0.0);
    assert!(score.get(Priority::FamilyStrong) > 1e90);
    assert!(Priority::FamilyStrong < Priority::Lang);
    assert!(Priority::Lang < Priority::FamilyWeak);
}

/// The whole point of the priority vector: an earlier slot decides outright.
#[test]
fn an_earlier_priority_outranks_every_later_one() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();

    // Right family, badly wrong weight, versus wrong family and exact weight.
    let right_family = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Weight, 123);
    });
    let wrong_family = query(|q| {
        q.add(Object::Family, "No Such Family");
        q.add(Object::Weight, 80);
    });
    let (_, good) = fontconf::best(&right_family, fonts.clone()).unwrap();
    let (_, bad) = fontconf::best(&wrong_family, fonts).unwrap();
    assert!(good.get(Priority::Weight) > bad.get(Priority::Weight));
    assert!(good.beats(&bad), "family must outrank weight");
}

/// A variable font's weight is a range, and a request inside it matches
/// exactly rather than by distance to an endpoint.
#[test]
fn a_range_contains_rather_than_approximates() {
    let cache = cantarell();
    let variable = cache
        .fonts()
        .unwrap()
        .find(|f| f.value(Object::Variable) == Some(fontconf::Value::Bool(true)))
        .expect("the fixture has a variable pattern");

    let inside = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Weight, 123); // within 0..205, but no instance has it
    });
    let score = fontconf::score(&inside, &variable).unwrap();
    assert_eq!(score.get(Priority::Weight), 0.0, "inside the range is an exact match");

    let outside = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Weight, 255); // 50 past the end
    });
    let score = fontconf::score(&outside, &variable).unwrap();
    assert_eq!(score.get(Priority::Weight), 50_000.0, "distance, scaled by 1000");
}

#[test]
fn scores_compare_lexicographically() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let q = query(|q| {
        q.add(Object::Family, "Cantarell");
        q.add(Object::Weight, 200);
    });
    let ranked = fontconf::sorted(&q, fonts);
    assert_eq!(ranked.len(), 6);
    assert_eq!(ranked[0].0.string(Object::Style), Some("Bold"));
    // Sorted really is sorted: each score is no worse than the next.
    for pair in ranked.windows(2) {
        assert!(!pair[1].1.beats(&pair[0].1), "sorted order violated");
    }
    // And `best` agrees with the head of the ranking.
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let (best, _) = fontconf::best(&q, fonts).unwrap();
    assert_eq!(best.string(Object::Style), ranked[0].0.string(Object::Style));
}

/// An exact tie keeps the font seen first, which is what makes the walk order
/// part of the answer.
#[test]
fn a_tie_keeps_the_earlier_font() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let first_style = fonts[0].string(Object::Style).map(str::to_string);

    // A query that says nothing distinguishing: every instance ties on family.
    let mut q = query(|q| {
        q.add(Object::Family, "Cantarell");
    });
    q.remove(Object::Weight);
    q.remove(Object::Slant);
    q.remove(Object::Width);
    q.remove(Object::Size);
    q.remove(Object::PixelSize);
    q.remove(Object::Fontversion);

    let (best, _) = fontconf::best(&q, fonts).unwrap();
    assert_eq!(best.string(Object::Style).map(str::to_string), first_style);
}

#[test]
fn a_score_of_all_zeroes_beats_nothing_and_loses_to_nothing() {
    let cache = cantarell();
    let fonts: Vec<_> = cache.fonts().unwrap().collect();
    let q = query(|q| {
        q.add(Object::Family, "Cantarell");
    });
    let score: Score = fontconf::score(&q, &fonts[0]).unwrap();
    assert!(!score.beats(&score), "a score must not beat itself");
    assert_eq!(score.as_slice().len(), fontconf::PRIORITIES);
}
