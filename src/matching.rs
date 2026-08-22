//! Scoring a font against a query, and picking the best one.
//!
//! Every font gets a vector of distances, one slot per [`Priority`]. The
//! winner is the font whose vector is smallest lexicographically: the first
//! slot where two fonts differ decides between them outright, so a better
//! family match beats any amount of weight mismatch.
//!
//! Only properties present on *both* sides contribute. A query saying nothing
//! about spacing scores nothing for spacing, and a font that never mentions
//! `color` is not penalised against a query that does.

use crate::casefold;
use crate::charset::CharSet;
use crate::glob;
use crate::langset;
use crate::object::Object;
use crate::pattern::Pattern;
use crate::query::{OwnedValue, Query};
use crate::value::{Binding, Value};

/// Where a property sits in the match priority order.
///
/// Transcribed from `FcMatcherPriority` in `fcmatch.c`, whose comment reads
/// "Order is significant, it defines the precedence of each value, earlier
/// values are more significant than later values".
///
/// Family and PostScript name appear twice: a strongly-bound value of either
/// outranks the language match, and a weakly-bound one does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum Priority {
    /// Distance for `file`.
    File,
    /// Distance for `fontwrapper`.
    FontWrapper,
    /// Distance for `fontformat`.
    FontFormat,
    /// Distance for `variable`.
    Variable,
    /// Distance for `namedinstance`.
    NamedInstance,
    /// Distance for `scalable`.
    Scalable,
    /// Distance for `color`.
    Color,
    /// Distance for `foundry`.
    Foundry,
    /// Distance for `charset`.
    CharSet,
    /// Distance for `family, strongly bound`.
    FamilyStrong,
    /// Distance for `postscriptname, strongly bound`.
    PostScriptNameStrong,
    /// Distance for `lang`.
    Lang,
    /// Distance for `family, weakly bound`.
    FamilyWeak,
    /// Distance for `postscriptname, weakly bound`.
    PostScriptNameWeak,
    /// Distance for `symbol`.
    Symbol,
    /// Distance for `spacing`.
    Spacing,
    /// Distance for `size`.
    Size,
    /// Distance for `pixelsize`.
    PixelSize,
    /// Distance for `style`.
    Style,
    /// Distance for `slant`.
    Slant,
    /// Distance for `weight`.
    Weight,
    /// Distance for `width`.
    Width,
    /// Distance for `fonthashint`.
    FontHasHint,
    /// Distance for `decorative`.
    Decorative,
    /// Distance for `antialias`.
    Antialias,
    /// Distance for `rasterizer`.
    Rasterizer,
    /// Distance for `outline`.
    Outline,
    /// Distance for `order`.
    Order,
    /// Distance for `fontversion`.
    FontVersion,
}

/// How many priority slots a score vector has, fontconfig's `PRI_END`.
pub const PRIORITIES: usize = Priority::FontVersion as usize + 1;

/// How well a font answered a query: one distance per [`Priority`].
///
/// Smaller is better, and comparison is lexicographic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score([f64; PRIORITIES]);

impl Score {
    fn zero() -> Self {
        Self([0.0; PRIORITIES])
    }

    /// The distance recorded for one priority.
    pub fn get(&self, priority: Priority) -> f64 {
        self.0[priority as usize]
    }

    /// The whole vector, in priority order.
    pub fn as_slice(&self) -> &[f64; PRIORITIES] {
        &self.0
    }

    /// Whether this score beats `other`.
    ///
    /// The first slot where the two differ decides. Fontconfig keeps the
    /// earlier font on an exact tie, so this is strictly "better than".
    pub fn beats(&self, other: &Score) -> bool {
        for (ours, theirs) in self.0.iter().zip(&other.0) {
            if ours < theirs {
                return true;
            }
            if ours > theirs {
                return false;
            }
        }
        false
    }
}

/// The "no match at all" distance, fontconfig's literal `1e99`.
///
/// Not `f64::MAX`: these values are summed, and `f64::MAX` plus anything is
/// infinity, which would make two differently-bad fonts compare equal.
const NO_MATCH: f64 = 1e99;

/// A distance between one query value and one font value, or `None` when the
/// two are not even the same kind of thing.
///
/// Fontconfig signals that with -1 and treats it as "this font cannot answer
/// the query at all", discarding the font rather than scoring it badly.
type Compare = fn(&Value<'_>, &Value<'_>) -> Option<f64>;

/// How one property is scored, and which slots it contributes to.
struct Matcher {
    compare: Compare,
    strong: Priority,
    weak: Priority,
}

/// The matcher for `object`, or `None` if it takes no part in scoring.
///
/// Most properties have no matcher at all -- `hintstyle`, `dpi`, `index` and
/// the rest are carried around but never compared.
fn matcher(object: Object) -> Option<Matcher> {
    use Priority as P;
    let (compare, strong, weak): (Compare, P, P) = match object {
        Object::Family => (compare_family, P::FamilyStrong, P::FamilyWeak),
        Object::PostscriptName => {
            (compare_postscript, P::PostScriptNameStrong, P::PostScriptNameWeak)
        }
        Object::Style => (compare_string, P::Style, P::Style),
        Object::Foundry => (compare_string, P::Foundry, P::Foundry),
        Object::Rasterizer => (compare_string, P::Rasterizer, P::Rasterizer),
        Object::Fontformat => (compare_string, P::FontFormat, P::FontFormat),
        Object::FontWrapper => (compare_string, P::FontWrapper, P::FontWrapper),
        Object::Slant => (compare_number, P::Slant, P::Slant),
        Object::PixelSize => (compare_number, P::PixelSize, P::PixelSize),
        Object::Spacing => (compare_number, P::Spacing, P::Spacing),
        Object::Fontversion => (compare_number, P::FontVersion, P::FontVersion),
        Object::Order => (compare_number, P::Order, P::Order),
        Object::Weight => (compare_range, P::Weight, P::Weight),
        Object::Width => (compare_range, P::Width, P::Width),
        Object::Size => (compare_size, P::Size, P::Size),
        Object::Antialias => (compare_bool, P::Antialias, P::Antialias),
        Object::Outline => (compare_bool, P::Outline, P::Outline),
        Object::Scalable => (compare_bool, P::Scalable, P::Scalable),
        Object::Decorative => (compare_bool, P::Decorative, P::Decorative),
        Object::Color => (compare_bool, P::Color, P::Color),
        Object::Symbol => (compare_bool, P::Symbol, P::Symbol),
        Object::Variable => (compare_bool, P::Variable, P::Variable),
        Object::FontHasHint => (compare_bool, P::FontHasHint, P::FontHasHint),
        Object::NamedInstance => (compare_bool, P::NamedInstance, P::NamedInstance),
        Object::File => (compare_filename, P::File, P::File),
        Object::Charset => (compare_charset, P::CharSet, P::CharSet),
        Object::Lang => (compare_lang, P::Lang, P::Lang),
        _ => return None,
    };
    Some(Matcher { compare, strong, weak })
}

// --- the individual comparisons -------------------------------------------
//
// Each returns a distance, or `None` for a type mismatch, which fontconfig
// signals with -1 and treats as "this font cannot answer at all".

fn compare_string(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    Some(f64::from(!casefold::eq(a, b)))
}

/// Families compare ignoring case *and* blanks, so `DejaVuSans` matches
/// `DejaVu Sans`.
///
/// The leading-character shortcut mirrors fontconfig: if the first characters
/// differ and neither is a space, the names cannot match, so it can skip the
/// full comparison.
fn compare_family(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    if !first_chars_could_match(a, b) {
        return Some(1.0);
    }
    Some(f64::from(!casefold::eq_ignoring_blanks(a, b)))
}

/// PostScript names compare as a fraction of characters shared, ignoring
/// case and the delimiters fontconfig lists as `" -,"`.
fn compare_postscript(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    if !first_chars_could_match(a, b) {
        return Some(1.0);
    }
    let shared = shared_prefix_ignoring_delimiters(a, b);
    let longest = a.len().max(b.len());
    if longest == 0 {
        return Some(0.0);
    }
    Some((longest - shared) as f64 / longest as f64)
}

fn first_chars_could_match(a: &str, b: &str) -> bool {
    let first = |s: &str| s.chars().next();
    match (first(a), first(b)) {
        (Some(x), Some(y)) => {
            x.to_lowercase().eq(y.to_lowercase()) || x == ' ' || y == ' '
        }
        _ => true,
    }
}

/// How many characters of `a` and `b` agree, ignoring case and skipping the
/// delimiters `' '`, `'-'` and `','` on either side.
///
/// This is `FcStrMatchIgnoreCaseAndDelims`.
fn shared_prefix_ignoring_delimiters(a: &str, b: &str) -> usize {
    const DELIMS: [char; 3] = [' ', '-', ','];
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    let mut shared = 0;
    loop {
        while a.peek().is_some_and(|c| DELIMS.contains(c)) {
            a.next();
        }
        while b.peek().is_some_and(|c| DELIMS.contains(c)) {
            b.next();
        }
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x.to_lowercase().eq(y.to_lowercase()) => shared += 1,
            _ => return shared,
        }
    }
}

fn compare_number(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    Some((number(b)? - number(a)?).abs())
}

fn number(value: &Value<'_>) -> Option<f64> {
    match value {
        Value::Int(i) => Some(f64::from(*i)),
        Value::Double(d) => Some(*d),
        _ => None,
    }
}

/// A value as the span it covers: a scalar is a span of zero width.
fn span(value: &Value<'_>) -> Option<(f64, f64)> {
    match value {
        Value::Int(i) => Some((f64::from(*i), f64::from(*i))),
        Value::Double(d) => Some((*d, *d)),
        Value::Range(r) => Some((r.begin, r.end)),
        _ => None,
    }
}

/// Overlapping spans match exactly; otherwise the distance is the gap.
fn compare_range(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let ((b1, e1), (b2, e2)) = (span(a)?, span(b)?);
    if e1 < b2 || e2 < b1 {
        Some((b2 - e1).abs().min((b1 - e2).abs()))
    } else {
        Some(0.0)
    }
}

/// Like [`compare_range`], but a size that sits exactly on the far end of a
/// span is nudged off zero.
///
/// The `1e-15` is fontconfig's, and its comment calls the span semi-closed:
/// it keeps a font whose range merely touches the requested size from tying
/// with one that genuinely contains it.
fn compare_size(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let ((b1, e1), (b2, e2)) = (span(a)?, span(b)?);
    if e1 < b2 || e2 < b1 {
        return Some((b2 - e1).abs().min((b1 - e2).abs()));
    }
    if b2 != e2 && b1 == e2 {
        return Some(1e-15);
    }
    Some(0.0)
}

fn compare_bool(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    match (a, b) {
        (Value::Bool(a), Value::Bool(b)) => Some(f64::from(a != b)),
        _ => None,
    }
}

/// How many characters the query wants that the font does not have.
fn compare_charset(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let (Value::CharSet(want), Value::CharSet(got)) = (a, b) else {
        return None;
    };
    Some(subtract_count(want, got) as f64)
}

fn subtract_count(want: &CharSet<'_>, got: &CharSet<'_>) -> usize {
    want.chars().filter(|c| !got.contains(*c)).count()
}

/// A language request scores by how close the font gets: the same language
/// is 0, the same language in another region is 1, and unrelated is 2.
///
/// Either side may be a langset or a plain tag, which is why this has four
/// arms rather than one.
fn compare_lang(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let result = match (a, b) {
        (Value::LangSet(a), Value::LangSet(b)) => a.compare(b),
        (Value::LangSet(a), Value::String(b)) => a.has_lang(b),
        (Value::String(a), Value::LangSet(b)) => b.has_lang(a),
        (Value::String(a), Value::String(b)) => langset::compare_lang(a, b),
        _ => return None,
    };
    Some(result as u8 as f64)
}

/// Filenames score by how loosely they match: identical, same but for case,
/// glob, or not at all.
fn compare_filename(a: &Value<'_>, b: &Value<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    Some(if a == b {
        0.0
    } else if a.eq_ignore_ascii_case(b) {
        1.0
    } else if glob::matches(a, b) {
        2.0
    } else {
        3.0
    })
}

// --- putting it together --------------------------------------------------

/// Score one property, adding into `score`.
///
/// The distance is combined with the positions of the two values in their
/// lists: `v * 1000 + j * 100 + k`, so a worse match on an earlier-listed
/// value still beats a better match on a later one. The `k` term only counts
/// for strings, where list order is a real preference rather than an accident.
fn score_values(
    matcher: &Matcher,
    query: &[(Value<'_>, Binding)],
    font: &[Value<'_>],
    score: &mut Score,
) -> bool {
    let (mut best, mut best_strong, mut best_weak) = (NO_MATCH, NO_MATCH, NO_MATCH);
    let split = matcher.strong != matcher.weak;

    'outer: for (j, (want, binding)) in query.iter().enumerate() {
        for (k, got) in font.iter().enumerate() {
            let Some(distance) = (matcher.compare)(want, got) else {
                return false;
            };
            let ordered = distance * 1000.0
                + j as f64 * 100.0
                + if matches!(got, Value::String(_)) { k as f64 } else { 0.0 };
            best = best.min(ordered);
            if !split {
                // An exact match on the first-listed value cannot be beaten.
                if best < 1000.0 {
                    break 'outer;
                }
            } else if *binding == Binding::Strong {
                best_strong = best_strong.min(ordered);
            } else {
                best_weak = best_weak.min(ordered);
            }
        }
    }

    if split {
        score.0[matcher.weak as usize] += best_weak;
        score.0[matcher.strong as usize] += best_strong;
    } else {
        score.0[matcher.strong as usize] += best;
    }
    true
}

/// Family is scored by *position*, not by distance.
///
/// Fontconfig builds a hash of the query's families and looks each of the
/// font's families up in it, so the score is the index of the earliest query
/// family the font also has -- and [`NO_MATCH`] if it has none. There is no
/// partial credit: a family either matches, ignoring case and blanks, or it
/// does not.
fn score_families(query: &[(Value<'_>, Binding)], font: &[Value<'_>], score: &mut Score) {
    let (mut strong, mut weak) = (NO_MATCH, NO_MATCH);
    for got in font {
        let Some(got) = got.as_str() else { continue };
        for (index, (want, binding)) in query.iter().enumerate() {
            let Some(want) = want.as_str() else { continue };
            if !casefold::eq_ignoring_blanks(want, got) {
                continue;
            }
            let index = index as f64;
            match binding {
                Binding::Weak => weak = weak.min(index),
                _ => strong = strong.min(index),
            }
        }
    }
    score.0[Priority::FamilyStrong as usize] = strong;
    score.0[Priority::FamilyWeak as usize] = weak;
}

/// Score `font` against `query`, or `None` if a property could not be
/// compared at all because the two sides disagreed about its type.
pub fn score(query: &Query, font: &Pattern<'_>) -> Option<Score> {
    let mut score = Score::zero();

    // Both sides are sorted by object id, so this is a merge join: only
    // properties they share are scored.
    let mut font_elements = font.elements().peekable();
    for element in query.elements() {
        let object = element.object();
        let font_element = loop {
            let next = font_elements.peek()?;
            match next.id().cmp(&object.id()) {
                std::cmp::Ordering::Less => {
                    font_elements.next();
                }
                std::cmp::Ordering::Greater => break None,
                std::cmp::Ordering::Equal => break font_elements.next(),
            }
        };
        let Some(font_element) = font_element else { continue };

        let wanted: Vec<(Value<'_>, Binding)> =
            element.values().map(|(v, b)| (v.as_value(), b)).collect();
        let got: Vec<Value<'_>> = font_element.values().collect();

        if object == Object::Family {
            score_families(&wanted, &got, &mut score);
            continue;
        }
        let Some(matcher) = matcher(object) else { continue };
        if !score_values(&matcher, &wanted, &got, &mut score) {
            return None;
        }
    }
    Some(score)
}

/// The best font for `query`, with its score.
///
/// Ties keep the font that came first, which is the order the caches were
/// walked in.
pub fn best<'a, I>(query: &Query, fonts: I) -> Option<(Pattern<'a>, Score)>
where
    I: IntoIterator<Item = Pattern<'a>>,
{
    let mut best: Option<(Pattern<'a>, Score)> = None;
    for font in fonts {
        let Some(score) = score(query, &font) else { continue };
        let better = match &best {
            None => true,
            Some((_, current)) => score.beats(current),
        };
        if better {
            best = Some((font, score));
        }
    }
    best
}

/// Which of a font's values best answered the query, and what it resolved to.
///
/// `FcCompareValueList` reports both: the index picks the localized name to
/// promote, and the value is what a prepared pattern carries. They usually
/// agree with "the font's value at that index", but a range collapses to a
/// concrete number chosen against the query.
pub struct BestValue {
    /// Index into the font's value list.
    pub index: usize,
    /// The number a range resolved to, if it was one.
    pub resolved: Option<f64>,
}

/// Find which font value answers `query` best for `object`.
pub fn best_value(
    query: &Query,
    font: &Pattern<'_>,
    object: Object,
) -> Option<BestValue> {
    let matcher = matcher(object)?;
    let element = query.get(object)?;
    let wanted: Vec<(Value<'_>, Binding)> =
        element.values().map(|(v, b)| (v.as_value(), b)).collect();
    let got: Vec<Value<'_>> = font.get(object)?.values().collect();

    let (mut best, mut index) = (f64::MAX, 0usize);
    for (j, (want, _)) in wanted.iter().enumerate() {
        for (k, value) in got.iter().enumerate() {
            let Some(distance) = (matcher.compare)(want, value) else {
                continue;
            };
            let ordered = distance * 1000.0
                + j as f64 * 100.0
                + if matches!(value, Value::String(_)) { k as f64 } else { 0.0 };
            if ordered < best {
                best = ordered;
                index = k;
            }
        }
    }

    // A range does not survive into a prepared pattern: fontconfig replaces
    // it with a single number pulled from the font's span towards the query's,
    // which is what gives a variable font a concrete weight.
    let resolved = match (wanted.first().map(|(v, _)| v), got.get(index)) {
        (Some(want), Some(got)) if matches!(object, Object::Weight | Object::Width) => {
            resolve_range(want, got)
        }
        // Size is the exception: it resolves to the midpoint of what was
        // *asked for*, not of what the font offers.
        (Some(want), _) if object == Object::Size => {
            span(want).map(|(b, e)| (b + e) * 0.5)
        }
        _ => None,
    };
    Some(BestValue { index, resolved })
}

/// The number a range comparison settles on, `FcCompareRange`'s `bestValue`.
fn resolve_range(want: &Value<'_>, got: &Value<'_>) -> Option<f64> {
    let ((b1, e1), (b2, e2)) = (span(want)?, span(got)?);
    // Only a real range needs resolving; a scalar is already a number.
    if !matches!(got, Value::Range(_)) {
        return None;
    }
    Some(if e1 < b2 {
        b2
    } else if e2 < b1 {
        e2
    } else {
        (b1.max(b2) + e1.min(e2)) * 0.5
    })
}

/// The score fontconfig assigns a font that satisfies no language the query
/// asked for.
///
/// Large enough to sink it below every font that does, but still finite, so
/// the demoted fonts stay ordered among themselves.
const LANG_UNSATISFIED: f64 = 10_000.0;

/// A language result worse than this means the font did not answer at all.
const LANG_ANSWERED: f64 = 2_000.0;

/// Every font, ordered best first, optionally trimmed.
///
/// This is `FcFontSetSort`, and it is not just [`score`] plus a sort. Two
/// passes shape the result into a fallback chain rather than a ranking:
///
/// 1. **Language satisfaction.** Walking in score order, a font keeps its
///    language score only if it answers a language the query asked for that
///    nothing before it already answered. Otherwise it is demoted. Without
///    this a query naming three languages gets fifty fonts that all cover the
///    first one.
/// 2. **Trimming**, when `trim` is set. A font is kept only if it draws a
///    character none of its predecessors could. This is what `fc-match -s`
///    reports and `fc-match -a` does not.
pub fn sort<'a, I>(query: &Query, fonts: I, trim: bool) -> Vec<(Pattern<'a>, Score)>
where
    I: IntoIterator<Item = Pattern<'a>>,
{
    let mut scored: Vec<(Pattern<'a>, Score)> = fonts
        .into_iter()
        .filter_map(|font| score(query, &font).map(|s| (font, s)))
        .collect();
    sort_by_score(&mut scored);

    satisfy_languages(query, &mut scored);
    sort_by_score(&mut scored);

    if !trim {
        return scored;
    }

    let mut coverage = crate::charset::Coverage::new();
    let mut kept = Vec::with_capacity(scored.len());
    for (font, score) in scored {
        // A font with no charset cannot be judged, and fontconfig skips it
        // outright rather than keeping it on faith.
        let Some(Value::CharSet(charset)) = font.value(Object::Charset) else {
            continue;
        };
        let adds = coverage.merge(&charset);
        if kept.is_empty() || adds {
            kept.push((font, score));
        }
    }
    kept
}

/// Demote every font that answers no language the query still needs.
///
/// Each pattern language can be satisfied once. A font that satisfies one
/// keeps its score and claims that language; a font that satisfies none has
/// its language slot pushed to [`LANG_UNSATISFIED`].
fn satisfy_languages(query: &Query, scored: &mut [(Pattern<'_>, Score)]) {
    let Some(element) = query.get(Object::Lang) else {
        return;
    };
    let wanted: Vec<OwnedValue> = element.values().map(|(v, _)| v.clone()).collect();
    if wanted.is_empty() {
        return;
    }
    let mut satisfied = vec![false; wanted.len()];

    for (font, score) in scored.iter_mut() {
        let mut satisfies = false;
        if score.get(Priority::Lang) < LANG_ANSWERED {
            // Only the font's *first* language value is consulted, which is
            // what fontconfig does.
            if let Some(font_lang) = font.value(Object::Lang) {
                for (index, want) in wanted.iter().enumerate() {
                    if satisfied[index] {
                        continue;
                    }
                    let distance = compare_lang(&want.as_value(), &font_lang);
                    if distance.is_some_and(|d| (0.0..2.0).contains(&d)) {
                        satisfied[index] = true;
                        satisfies = true;
                        break;
                    }
                }
            }
        }
        if !satisfies {
            score.0[Priority::Lang as usize] = LANG_UNSATISFIED;
        }
    }
}

/// Order by the score vector, keeping equal scores in the order they arrived.
fn sort_by_score(scored: &mut [(Pattern<'_>, Score)]) {
    scored.sort_by(|(_, a), (_, b)| {
        a.0.iter()
            .zip(&b.0)
            .find_map(|(x, y)| x.partial_cmp(y).filter(|o| o.is_ne()))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Every font, ordered best first, without trimming.
///
/// Equivalent to [`sort`] with `trim` unset.
pub fn sorted<'a, I>(query: &Query, fonts: I) -> Vec<(Pattern<'a>, Score)>
where
    I: IntoIterator<Item = Pattern<'a>>,
{
    sort(query, fonts, false)
}
