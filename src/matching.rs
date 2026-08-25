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
use crate::charset::AnyCharSet;
use crate::fnv::BuildPassthrough;
use crate::glob;
use crate::langset;
use crate::object::Object;
use crate::pattern::PatternRef;
use crate::pattern::Values;
use crate::pattern::{Element, Pattern};
use crate::value::Value;
use crate::value::{Binding, ValueRef};
use std::collections::HashMap;

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

    /// The binding every value of `object` takes in the font that scored this.
    ///
    /// Matching does not only pick a font, it also decides how firmly that
    /// font holds each of its properties. `FcFontSetMatchInternal` rebuilds
    /// the winner before handing it to `FcFontRenderPrepare`, and gives each
    /// object one binding for all of its values: **strong** when the object's
    /// strong distance came in under 1000 -- fontconfig's threshold for "this
    /// matched exactly" -- and **weak** otherwise.
    ///
    /// `None` means the object has no matcher, and so is not rebound at all:
    /// it keeps whatever binding it already had. That is not a rare corner --
    /// `fullname`, `capability`, `fontvariations`, `matrix` and the whole
    /// rendering group are all in it, and values read from a cache are weak,
    /// because upstream never serializes this field and zeroes the block.
    ///
    /// A query for `DejaVu Sans` gets `family` back strongly bound; the same
    /// font reached through `sans-serif` gets it weakly, since the name that
    /// won was contributed by an alias rather than asked for.
    pub fn binding(&self, object: Object) -> Option<Binding> {
        let matcher = matcher(object)?;
        Some(if self.0[matcher.strong as usize] < 1000.0 { Binding::Strong } else { Binding::Weak })
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
type Compare = fn(&ValueRef<'_>, &ValueRef<'_>) -> Option<f64>;

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

fn compare_string(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    Some(f64::from(!casefold::eq(a, b)))
}

/// Families compare ignoring case *and* blanks, so `DejaVuSans` matches
/// `DejaVu Sans`.
///
/// The leading-character shortcut mirrors fontconfig: if the first characters
/// differ and neither is a space, the names cannot match, so it can skip the
/// full comparison.
fn compare_family(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    let (a, b) = (a.as_str()?, b.as_str()?);
    if !first_chars_could_match(a, b) {
        return Some(1.0);
    }
    Some(f64::from(!casefold::eq_ignoring_blanks(a, b)))
}

/// PostScript names compare as a fraction of characters shared, ignoring
/// case and the delimiters fontconfig lists as `" -,"`.
fn compare_postscript(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
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
        (Some(x), Some(y)) => x.to_lowercase().eq(y.to_lowercase()) || x == ' ' || y == ' ',
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

fn compare_number(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    Some((number(b)? - number(a)?).abs())
}

fn number(value: &ValueRef<'_>) -> Option<f64> {
    match value {
        ValueRef::Int(i) => Some(f64::from(*i)),
        ValueRef::Double(d) => Some(*d),
        _ => None,
    }
}

/// A value as the span it covers: a scalar is a span of zero width.
fn span(value: &ValueRef<'_>) -> Option<(f64, f64)> {
    match value {
        ValueRef::Int(i) => Some((f64::from(*i), f64::from(*i))),
        ValueRef::Double(d) => Some((*d, *d)),
        ValueRef::Range(r) => Some((r.begin, r.end)),
        _ => None,
    }
}

/// Overlapping spans match exactly; otherwise the distance is the gap.
fn compare_range(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
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
fn compare_size(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    let ((b1, e1), (b2, e2)) = (span(a)?, span(b)?);
    if e1 < b2 || e2 < b1 {
        return Some((b2 - e1).abs().min((b1 - e2).abs()));
    }
    if b2 != e2 && b1 == e2 {
        return Some(1e-15);
    }
    Some(0.0)
}

fn compare_bool(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    match (a, b) {
        // `(v2->u.b ^ v1->u.b) == 1`, which is not `a != b`: exclusive-or on
        // the integers means `DontCare` -- 2 -- differs from both states by
        // more than one bit and so scores as a match against either. That is
        // what makes it mean "either answer will do" rather than "a third
        // answer nothing has".
        (ValueRef::Bool(a), ValueRef::Bool(b)) => Some(f64::from((a.as_i32() ^ b.as_i32()) == 1)),
        _ => None,
    }
}

/// How many characters the query wants that the font does not have.
fn compare_charset(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    let (ValueRef::CharSet(want), ValueRef::CharSet(got)) = (a, b) else {
        return None;
    };
    Some(subtract_count(want, got) as f64)
}

fn subtract_count(want: &AnyCharSet<'_>, got: &AnyCharSet<'_>) -> usize {
    match got {
        // The font's coverage is the side worth resolving once: it is read
        // out of a cache, and the query names only a few characters.
        AnyCharSet::Cached(set) => set.missing_count(want.chars()),
        AnyCharSet::Owned(set) => want.chars().filter(|c| !set.contains(*c)).count(),
    }
}

/// A language request scores by how close the font gets: the same language
/// is 0, the same language in another region is 1, and unrelated is 2.
///
/// Either side may be a langset or a plain tag, which is why this has four
/// arms rather than one.
fn compare_lang(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
    let result = match (a, b) {
        (ValueRef::LangSet(a), ValueRef::LangSet(b)) => a.compare(b),
        (ValueRef::LangSet(a), ValueRef::String(b)) => a.has_lang(b),
        (ValueRef::String(a), ValueRef::LangSet(b)) => b.has_lang(a),
        (ValueRef::String(a), ValueRef::String(b)) => langset::compare_lang(a, b),
        _ => return None,
    };
    Some(result as u8 as f64)
}

/// Filenames score by how loosely they match: identical, same but for case,
/// glob, or not at all.
fn compare_filename(a: &ValueRef<'_>, b: &ValueRef<'_>) -> Option<f64> {
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
fn score_values(matcher: &Matcher, query: &Element, font: &Values<'_>, score: &mut Score) -> bool {
    let (mut best, mut best_strong, mut best_weak) = (NO_MATCH, NO_MATCH, NO_MATCH);
    let split = matcher.strong != matcher.weak;

    // Neither side is collected. The font's values are re-walked per query
    // value by cloning the cursor, which is three words; collecting them
    // instead would allocate twice for every property of every font, on
    // every match.
    'outer: for (j, (want, binding)) in query.values().enumerate() {
        let want = want.as_value();
        for (k, got) in font.clone().enumerate() {
            let Some(distance) = (matcher.compare)(&want, &got) else {
                return false;
            };
            let ordered = distance * 1000.0
                + j as f64 * 100.0
                + if matches!(got, ValueRef::String(_)) { k as f64 } else { 0.0 };
            best = best.min(ordered);
            if !split {
                // An exact match on the first-listed value cannot be beaten.
                if best < 1000.0 {
                    break 'outer;
                }
            } else if binding == Binding::Strong {
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

/// The query's families, indexed by name.
///
/// This is `FcCompareDataInit`, and it exists for one reason: a query is not
/// one family. Configuration expands `sans-serif` -- or anything else --
/// through the alias chain, so a prepared query here carries between 76 and
/// 142 of them. Comparing each against each of a font's families, for every
/// font, is a hundred thousand case-folding string comparisons per match.
/// Building this once and looking each font family up turns that into two or
/// three lookups per font.
///
/// Keyed by a hash rather than by an owned folded string so that a lookup
/// allocates nothing, and the hash is used as it stands rather than being
/// hashed again. A hit is confirmed with the real comparison, because equal
/// hashes are not equal names.
///
/// The entries live in one flat list, chained by index, because a map of
/// small `Vec`s would allocate once per distinct family -- over a hundred
/// times per match, to hold one element each.
struct Families<'q> {
    heads: HashMap<u64, u32, BuildPassthrough>,
    entries: Vec<Family<'q>>,
}

/// One family the query asked for, and how early it asked.
struct Family<'q> {
    name: &'q str,
    /// The index of the earliest strongly-bound mention, or [`NO_MATCH`].
    strong: f64,
    /// The same for weakly-bound ones.
    weak: f64,
    /// The next entry whose name hashes the same, if any.
    next: Option<u32>,
}

impl<'q> Families<'q> {
    /// Index the family element of `query`, if it has one.
    fn new(query: &'q Pattern) -> Self {
        let mut families = Self { heads: HashMap::default(), entries: Vec::new() };
        let Some(element) = query.get(Object::Family) else { return families };
        families.entries.reserve(element.values().count());

        for (index, (want, binding)) in element.values().enumerate() {
            let Value::String(name) = want else { continue };
            let index = index as f64;
            let hash = casefold::hash_ignoring_blanks(name);

            // The same name can appear twice with different bindings, and
            // only the earliest mention of each counts.
            let existing = families
                .chain(hash)
                .find(|at| casefold::eq_ignoring_blanks(families.entries[*at as usize].name, name));
            let at = match existing {
                Some(at) => at as usize,
                None => {
                    let at = families.entries.len() as u32;
                    families.entries.push(Family {
                        name,
                        strong: NO_MATCH,
                        weak: NO_MATCH,
                        next: families.heads.insert(hash, at),
                    });
                    at as usize
                }
            };
            let entry = &mut families.entries[at];
            match binding {
                Binding::Weak => entry.weak = entry.weak.min(index),
                _ => entry.strong = entry.strong.min(index),
            }
        }
        families
    }

    /// The entries whose names hash to `hash`, newest first.
    fn chain(&self, hash: u64) -> impl Iterator<Item = u32> + '_ {
        let mut next = self.heads.get(&hash).copied();
        std::iter::from_fn(move || {
            let at = next?;
            next = self.entries[at as usize].next;
            Some(at)
        })
    }

    /// Where the query asked for `name`, if it did.
    fn find(&self, name: &str) -> Option<&Family<'q>> {
        self.chain(casefold::hash_ignoring_blanks(name))
            .map(|at| &self.entries[at as usize])
            .find(|entry| casefold::eq_ignoring_blanks(entry.name, name))
    }
}

/// Coverage, scored against characters extracted once for the query.
///
/// The same shape as [`score_values`] for a matcher with one priority. What
/// is lifted out is the query's own character list: it lives in a page
/// bitmap, and walking it back out for every font in the set was most of
/// what a fallback query cost.
fn score_charsets(chars: &[Vec<char>], font: &Values<'_>, score: &mut Score) -> bool {
    let mut best = NO_MATCH;
    'outer: for (j, want) in chars.iter().enumerate() {
        for got in font.clone() {
            let ValueRef::CharSet(got) = got else { return false };
            let missing = match got {
                AnyCharSet::Cached(set) => set.missing_count(want.iter().copied()),
                AnyCharSet::Owned(set) => want.iter().filter(|c| !set.contains(**c)).count(),
            };
            // A charset never scores by position within the font, so there
            // is no `k` term here: see [`score_values`].
            best = best.min(missing as f64 * 1000.0 + j as f64 * 100.0);
            if best < 1000.0 {
                break 'outer;
            }
        }
    }
    score.0[Priority::CharSet as usize] += best;
    true
}

/// Language, scored against ranks worked out once for the query.
///
/// The same shape as [`score_values`] for a matcher whose two priorities are
/// the same -- language has no strong and weak halves -- with the table
/// search lifted out. The arms that are not a tag against a set fall back to
/// the general comparison, which is what a query carrying a language *set*
/// takes.
fn score_langs(ranks: &[LangRank], query: &Element, font: &Values<'_>, score: &mut Score) -> bool {
    let mut best = NO_MATCH;
    'outer: for (j, (want, _binding)) in query.values().enumerate() {
        let want = want.as_value();
        for got in font.clone() {
            let result = match (&want, &got, ranks.get(j)) {
                (ValueRef::String(tag), ValueRef::LangSet(set), Some(rank)) => {
                    set.has_lang_from(tag, *rank) as u8 as f64
                }
                _ => match compare_lang(&want, &got) {
                    Some(distance) => distance,
                    None => return false,
                },
            };
            // A language set never scores by position within the font, so
            // there is no `k` term here: see [`score_values`].
            best = best.min(result * 1000.0 + j as f64 * 100.0);
            if best < 1000.0 {
                break 'outer;
            }
        }
    }
    score.0[Priority::Lang as usize] += best;
    true
}

/// Family is scored by *position*, not by distance.
///
/// The score is the index of the earliest query family the font also has --
/// and [`NO_MATCH`] if it has none. There is no partial credit: a family
/// either matches, ignoring case and blanks, or it does not.
fn score_families(families: &Families<'_>, font: &Values<'_>, score: &mut Score) {
    let (mut strong, mut weak) = (NO_MATCH, NO_MATCH);
    for got in font.clone() {
        let Some(got) = got.as_str() else { continue };
        if let Some(found) = families.find(got) {
            strong = strong.min(found.strong);
            weak = weak.min(found.weak);
        }
    }
    score.0[Priority::FamilyStrong as usize] = strong;
    score.0[Priority::FamilyWeak as usize] = weak;
}

/// Score `font` against `query`, or `None` if a property could not be
/// compared at all because the two sides disagreed about its type.
pub fn score(query: &Pattern, font: &PatternRef<'_>) -> Option<Score> {
    // The one-shot form works the query out for a single font, which is
    // wasted effort repeated. Anything scoring more than one font should
    // prepare it once: see [`best`] and [`sort`].
    score_prepared(&Prepared::new(query), font)
}

/// A query with everything that depends only on the query worked out.
///
/// Scoring runs this against every font in the set, so anything computed
/// from the query alone is computed hundreds of times unless it is lifted
/// out. The family index is the big one; the matcher lookup is the small one
/// that turned out to matter, because it was an indirect call through a
/// function pointer for every property of every font.
struct Prepared<'q> {
    families: Families<'q>,
    elements: Vec<Prepped<'q>>,
    /// Where each language the query names sits in the table, in value
    /// order. Finding that is a binary search over three hundred names, and
    /// it was being repeated for every font in the set.
    langs: Vec<LangRank>,
}

/// The rank of one language the query asked for: its place in the table, or
/// where it would go.
type LangRank = std::result::Result<usize, usize>;

/// One property of the query, ready to score against.
struct Prepped<'q> {
    /// The object id, so the walk over properties compares integers.
    id: i32,
    element: &'q Element,
    how: How,
}

/// How one property is scored.
///
/// Three properties are worth working out in advance, and each was a
/// separate flag on the way here. They have the same shape -- something
/// derived from the query alone, which the general path would recompute for
/// every font -- so they are one choice rather than three tests.
enum How {
    /// Against the family index.
    Families,
    /// Against the language ranks.
    LangSet,
    /// Against the query characters, already extracted, one list per value.
    CharSets(Vec<Vec<char>>),
    /// The general path, through the matcher.
    Values(Matcher),
    /// A property that takes no part in scoring at all.
    Skip,
}

impl<'q> Prepared<'q> {
    fn new(query: &'q Pattern) -> Self {
        let elements = query
            .elements()
            .map(|element| {
                let object = element.object();
                let how = match object {
                    Object::Family => How::Families,
                    Object::Lang => How::LangSet,
                    Object::Charset => {
                        // Extracting up front is what makes charset scoring
                        // cheap: the characters are walked once here rather
                        // than out of the query's page bitmap for every font
                        // in the set.
                        //
                        // It only holds if every value really is a charset.
                        // One that is not has no characters to extract, and
                        // an empty list reads as "wants nothing", which
                        // scores zero -- a perfect charset match against
                        // every font. Those go to the general comparison,
                        // which rejects a type it cannot compare.
                        let extracted: Option<Vec<Vec<char>>> = element
                            .values()
                            .map(|(value, _)| match value {
                                Value::CharSet(set) => Some(set.chars().collect()),
                                _ => None,
                            })
                            .collect();
                        match (extracted, matcher(object)) {
                            (Some(lists), _) => How::CharSets(lists),
                            (None, Some(matcher)) => How::Values(matcher),
                            (None, None) => How::Skip,
                        }
                    }
                    other => match matcher(other) {
                        Some(matcher) => How::Values(matcher),
                        None => How::Skip,
                    },
                };
                Prepped { id: object.id(), element, how }
            })
            .collect();
        let langs = query
            .get(Object::Lang)
            .map(|element| {
                element
                    .values()
                    .map(|(value, _)| match value {
                        Value::String(name) => crate::langs::rank_of(name),
                        // Not a tag, so nothing to look up; the comparison
                        // takes its other arm.
                        _ => Err(0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { families: Families::new(query), elements, langs }
    }
}

/// [`score`], with the query already prepared.
fn score_prepared(query: &Prepared<'_>, font: &PatternRef<'_>) -> Option<Score> {
    let mut score = Score::zero();

    // Both sides are sorted by object id, so the two are walked in lockstep
    // and only properties they share are scored: a merge join.
    //
    // Nothing is copied out of either side: this runs once per font per
    // match, and a `Vec` here was the difference between microseconds and
    // tens of them.
    //
    // Walked by index rather than with iterators, because the skipped
    // elements are the common case and constructing a cursor for each one
    // only to compare its id and drop it is most of the loop.
    let count = font.len();
    let mut at = 0usize;
    for prepped in &query.elements {
        while at < count && font.element_id(at) < prepped.id {
            at += 1;
        }
        // The font running out of properties ends the join, exactly as
        // `FcCompare` ends its `while` loop: what has been scored so far
        // stands. Treating it as a failure to match would drop a font for
        // saying nothing, rather than for saying the wrong thing.
        if at >= count {
            return Some(score);
        }
        if font.element_id(at) != prepped.id {
            continue;
        }
        let got = font.element_values(at);
        at += 1;

        let scored = match &prepped.how {
            How::Families => {
                score_families(&query.families, &got, &mut score);
                true
            }
            How::LangSet => score_langs(&query.langs, prepped.element, &got, &mut score),
            How::CharSets(chars) => score_charsets(chars, &got, &mut score),
            How::Values(matcher) => score_values(matcher, prepped.element, &got, &mut score),
            How::Skip => true,
        };
        if !scored {
            return None;
        }
    }
    Some(score)
}

/// The best font for `query`, with its score.
///
/// Ties keep the font that came first, which is the order the caches were
/// walked in.
pub fn best<'a, I>(query: &Pattern, fonts: I) -> Option<(PatternRef<'a>, Score)>
where
    I: IntoIterator<Item = PatternRef<'a>>,
{
    let prepared = Prepared::new(query);
    let mut best: Option<(PatternRef<'a>, Score)> = None;
    for font in fonts {
        let Some(score) = score_prepared(&prepared, &font) else { continue };
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
    /// What the winning pair resolved to, where that is not simply the font's
    /// value: a range collapses to a number, and a font that says `DontCare`
    /// takes the query's answer instead of imposing its own.
    pub resolved: Option<Value>,
}

/// Find which font value answers `query` best for `object`.
pub fn best_value(query: &Pattern, font: &PatternRef<'_>, object: Object) -> Option<BestValue> {
    // `FcObjectToMatcher (object, include_lang = FcTrue)`. The name-language
    // objects have no comparison of their own; they borrow `lang`'s, which is
    // how a query asking for `familylang=ja` picks the Japanese name out of a
    // font that lists several. Upstream passes `FcTrue` at exactly one call
    // site, and it is the one that computes a best value.
    //
    // Only the comparison is borrowed. The values still come from the object
    // that was asked about -- reading `lang` here would compare the languages
    // the font can *write* against the languages its names are written in.
    let matcher = matcher(match object {
        Object::Familylang | Object::Stylelang | Object::Fullnamelang => Object::Lang,
        other => other,
    })?;
    let element = query.get(object)?;
    let wanted: Vec<(ValueRef<'_>, Binding)> =
        element.values().map(|(v, b)| (v.as_value(), b)).collect();
    let got: Vec<ValueRef<'_>> = font.get(object)?.values().collect();

    // Which font value won, and which *query* value it won against.
    // `FcCompareValueList` keeps the `bestValue` its winning pair produced,
    // so both indices matter: resolving against the first query value instead
    // answers `weight=300,150` with 205 where fontconfig answers 150.
    let (mut best, mut index, mut chosen) = (f64::MAX, 0usize, 0usize);
    for (j, (want, _)) in wanted.iter().enumerate() {
        for (k, value) in got.iter().enumerate() {
            let Some(distance) = (matcher.compare)(want, value) else {
                continue;
            };
            let ordered = distance * 1000.0
                + j as f64 * 100.0
                + if matches!(value, ValueRef::String(_)) { k as f64 } else { 0.0 };
            if ordered < best {
                best = ordered;
                index = k;
                chosen = j;
            }
        }
    }

    let resolved = match (wanted.get(chosen).map(|(v, _)| v), got.get(index)) {
        // A range does not survive into a prepared pattern: fontconfig
        // replaces it with a number pulled from the font's span towards the
        // query's, which is what gives a variable font a concrete weight.
        (Some(want), Some(got)) if matches!(object, Object::Weight | Object::Width) => {
            resolve_range(want, got).map(Value::Double)
        }
        // Size is the exception: `FcCompareSize` resolves to the midpoint of
        // what was *asked for*, not of what the font offers.
        (Some(want), _) if object == Object::Size => {
            span(want).map(|(b, e)| Value::Double((b + e) * 0.5))
        }
        // `FcCompareBool` keeps the font's answer unless the font has none:
        // `DontCare` means "either will do", so the query's answer stands.
        // Without this a prepared pattern hands a renderer a tri-state where
        // fontconfig always resolves to the caller's boolean.
        (Some(ValueRef::Bool(want)), Some(ValueRef::Bool(got))) => {
            Some(Value::Bool(if *got != crate::value::Tristate::DontCare { *got } else { *want }))
        }
        _ => None,
    };
    Some(BestValue { index, resolved })
}

/// The number a range comparison settles on, `FcCompareRange`'s `bestValue`.
fn resolve_range(want: &ValueRef<'_>, got: &ValueRef<'_>) -> Option<f64> {
    let ((b1, e1), (b2, e2)) = (span(want)?, span(got)?);
    // Only a real range needs resolving; a scalar is already a number.
    if !matches!(got, ValueRef::Range(_)) {
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
pub fn sort<'a, I>(query: &Pattern, fonts: I, trim: bool) -> Vec<(PatternRef<'a>, Score)>
where
    I: IntoIterator<Item = PatternRef<'a>>,
{
    let prepared = Prepared::new(query);
    let mut scored: Vec<(PatternRef<'a>, Score)> = fonts
        .into_iter()
        .filter_map(|font| score_prepared(&prepared, &font).map(|s| (font, s)))
        .collect();

    // What gets ordered is a list of indices, not the fonts. A scored font is
    // 264 bytes -- the score alone is 29 doubles -- and there are two passes
    // over a few thousand of them, so sorting them in place moved a quarter
    // of a kilobyte per swap to decide something four bytes wide.
    let mut order: Vec<u32> = (0..scored.len() as u32).collect();
    sort_order(&mut order, &scored);
    satisfy_languages(query, &order, &mut scored);
    sort_order(&mut order, &scored);

    if !trim {
        return order.iter().map(|&i| scored[i as usize]).collect();
    }

    // Trimming reads the order directly, so the gather above never happens
    // on this path: the fonts that survive are copied, and the rest are not.
    let mut coverage = crate::charset::CharSet::new();
    let mut kept = Vec::new();
    for &index in &order {
        let (font, score) = scored[index as usize];
        // A font with no charset cannot be judged, and fontconfig skips it
        // outright rather than keeping it on faith.
        let Some(ValueRef::CharSet(charset)) = font.value(Object::Charset) else {
            continue;
        };
        let adds = coverage.merge_chars(&charset);
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
fn satisfy_languages(query: &Pattern, order: &[u32], scored: &mut [(PatternRef<'_>, Score)]) {
    let Some(element) = query.get(Object::Lang) else {
        return;
    };
    let wanted: Vec<Value> = element.values().map(|(v, _)| v.clone()).collect();
    if wanted.is_empty() {
        return;
    }
    let mut satisfied = vec![false; wanted.len()];

    // In score order, which is what makes "the first font to answer a
    // language claims it" mean the best one.
    for &index in order {
        let (font, score) = &mut scored[index as usize];
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

/// Order the indices by the score each one names.
///
/// Stable, so fonts with equal scores stay in the order they arrived -- which
/// is the order the caches were walked in, and what fontconfig keeps.
fn sort_order(order: &mut [u32], scored: &[(PatternRef<'_>, Score)]) {
    order.sort_by(|&left, &right| {
        let (a, b) = (&scored[left as usize].1, &scored[right as usize].1);
        a.0.iter()
            .zip(&b.0)
            .find_map(|(x, y)| x.partial_cmp(y).filter(|o| o.is_ne()))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Every font, ordered best first, without trimming.
///
/// Equivalent to [`sort`] with `trim` unset.
pub fn sorted<'a, I>(query: &Pattern, fonts: I) -> Vec<(PatternRef<'a>, Score)>
where
    I: IntoIterator<Item = PatternRef<'a>>,
{
    sort(query, fonts, false)
}

/// Tests for the family index, which replaced a scan and has to agree with
/// it exactly.
#[cfg(test)]
mod family_tests {
    use super::{Families, NO_MATCH};
    use crate::{Binding, Object, Pattern};

    fn query(families: &[(&str, Binding)]) -> Pattern {
        let mut query = Pattern::new();
        for (name, binding) in families {
            query.add_with_binding(Object::Family, *name, *binding);
        }
        query
    }

    #[test]
    fn a_family_is_found_by_its_position() {
        let q = query(&[("First", Binding::Strong), ("Second", Binding::Strong)]);
        let families = Families::new(&q);
        assert_eq!(families.find("First").map(|f| f.strong), Some(0.0));
        assert_eq!(families.find("Second").map(|f| f.strong), Some(1.0));
        assert!(families.find("Third").is_none());
    }

    /// The lookup ignores case and blanks, exactly as the scan it replaced
    /// did: `FcStrCmpIgnoreBlanksAndCase`.
    #[test]
    fn lookup_ignores_case_and_blanks() {
        let q = query(&[("DejaVu Sans", Binding::Strong)]);
        let families = Families::new(&q);
        for spelling in ["DejaVu Sans", "dejavusans", "DEJAVU  SANS", "d e j a v u s a n s"] {
            assert!(families.find(spelling).is_some(), "{spelling}");
        }
        assert!(families.find("DejaVu Serif").is_none());
    }

    /// A blank is only a space. A tab is part of the name, which is what
    /// makes this a different question from trimming whitespace.
    #[test]
    fn a_tab_is_not_a_blank() {
        let q = query(&[("DejaVu Sans", Binding::Strong)]);
        assert!(Families::new(&q).find("DejaVu\tSans").is_none());
    }

    /// The two bindings are tracked apart, because they score into different
    /// priority slots.
    #[test]
    fn strong_and_weak_are_kept_apart() {
        let mut q = Pattern::new();
        q.add(Object::Family, "Strong Only");
        q.add_weak(Object::Family, "Weak Only");
        let families = Families::new(&q);

        let strong = families.find("Strong Only").expect("strong");
        assert_eq!((strong.strong, strong.weak), (0.0, NO_MATCH));
        let weak = families.find("Weak Only").expect("weak");
        assert_eq!((weak.strong, weak.weak), (NO_MATCH, 1.0));
    }

    /// One name mentioned twice keeps the earliest of each binding, which is
    /// what the `min` in the scan did.
    #[test]
    fn a_repeated_name_keeps_the_earliest_of_each() {
        let mut q = Pattern::new();
        q.add(Object::Family, "Filler");
        q.add_weak(Object::Family, "Repeated");
        q.add(Object::Family, "repeated");
        q.add_weak(Object::Family, "REPEATED");
        let families = Families::new(&q);
        let found = families.find("Repeated").expect("found");
        assert_eq!(found.weak, 1.0, "the earlier weak mention wins");
        assert_eq!(found.strong, 2.0, "and the strong one is tracked apart");
    }

    #[test]
    fn a_query_with_no_family_finds_nothing() {
        let mut q = Pattern::new();
        q.add(Object::Weight, 80);
        assert!(Families::new(&q).find("Anything").is_none());
    }

    /// Two names that hash together must not be confused for each other.
    /// The bucket is searched with the real comparison for this reason.
    #[test]
    fn a_hash_collision_does_not_match_the_wrong_name() {
        let q = query(&[("Alpha", Binding::Strong), ("Beta", Binding::Strong)]);
        let families = Families::new(&q);
        // Both are in the arena, and the names -- not the hashes -- decide.
        let names: Vec<_> = families.entries.iter().map(|f| f.name).collect();
        assert_eq!(names, ["Alpha", "Beta"]);
        assert_eq!(families.find("Alpha").map(|f| f.strong), Some(0.0));
        assert_eq!(families.find("Beta").map(|f| f.strong), Some(1.0));
    }
}

#[cfg(test)]
mod bool_score_tests {
    use super::compare_bool;
    use crate::value::{Tristate, ValueRef};

    fn score(font: Tristate, query: Tristate) -> Option<f64> {
        compare_bool(&ValueRef::Bool(font), &ValueRef::Bool(query))
    }

    /// `FcCompareBool` is `(v2->u.b ^ v1->u.b) == 1`, and the exclusive-or is
    /// load-bearing. `a != b` would score `DontCare` against `true` as a
    /// mismatch; the xor makes it 3, not 1, so it matches. That is what
    /// "either answer will do" is made of.
    #[test]
    fn dontcare_matches_whichever_way_the_font_answers() {
        use Tristate::{DontCare, False, True};
        assert_eq!(score(True, True), Some(0.0));
        assert_eq!(score(False, False), Some(0.0));
        assert_eq!(score(True, False), Some(1.0));
        assert_eq!(score(False, True), Some(1.0));

        assert_eq!(score(True, DontCare), Some(0.0), "a query that does not care");
        assert_eq!(score(False, DontCare), Some(0.0));
        assert_eq!(score(DontCare, True), Some(0.0), "a font that does not say");
        assert_eq!(score(DontCare, False), Some(0.0));
        assert_eq!(score(DontCare, DontCare), Some(0.0));
    }
}
