//! The languages a font can write.
//!
//! Fontconfig decides this by checking a font's coverage against an
//! orthography per language, and stores the answer as a bitmap over its own
//! language list. See [`langs`](crate::langs) for why that list is an
//! assumption about whichever fontconfig wrote the cache.

use crate::bytes::Bytes;
use crate::error::Result;
use crate::langs::{self, LANGS};

use crate::layout::NATIVE as L;

/// How close two languages are, fontconfig's `FcLangResult`.
///
/// The ordering is the point: `Equal` is better than `DifferentTerritory`,
/// which is better than `DifferentLang`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LangResult {
    /// The same language.
    Equal = 0,
    /// The same language, a different region: `en-US` against `en-GB`.
    DifferentTerritory = 1,
    /// Unrelated languages.
    DifferentLang = 2,
}

/// The set of languages a font supports, read from a cache.
///
/// One of three types for the same idea, told apart by where the bitmap
/// lives: this borrows it from a cache, [`LangSet`] holds its own and
/// can grow, and [`AnyLangSet`] is either of those seen through a reference.
#[derive(Clone, Copy)]
pub struct LangSetRef<'a> {
    pub(crate) data: Bytes<'a>,
    pub(crate) at: usize,
}

impl<'a> LangSetRef<'a> {
    /// How many 32-bit words the stored bitmap has.
    ///
    /// Compare against [`langs::MAP_WORDS`] to see whether the writer sized
    /// its language list the same way we do.
    pub fn map_words(&self) -> usize {
        let words = self.data.u32(self.at + L.map_size).unwrap_or(0) as usize;
        // Proved to fit before it is handed out: a map claiming four billion
        // words would make `word * 4` overflow a 32-bit `usize` before the
        // read that would have rejected it. A map that does not fit is no
        // map, so this reports none rather than however many happen to be
        // readable.
        self.data.array(self.at + L.map, words, 4).unwrap_or(0)
    }

    /// Whether bit `index` is set.
    pub fn contains_index(&self, index: usize) -> bool {
        let word = index / 32;
        if word >= self.map_words() {
            return false;
        }
        let Ok(bits) = self.data.u32(self.at + L.map + word * 4) else {
            return false;
        };
        bits & (1 << (index % 32)) != 0
    }

    /// Whether the font covers exactly this language, by name.
    ///
    /// This is an exact table lookup. Use [`LangSetRef::has_lang`] to ask the
    /// question fontconfig actually scores, which treats `en-US` and `en` as
    /// near-misses rather than as unrelated.
    pub fn contains(&self, lang: &str) -> bool {
        langs::index_of(lang).is_some_and(|i| self.contains_index(i))
    }

    /// Every language in the set, in bit order.
    ///
    /// That is the order `fc-list` prints them in: `FcNameUnparseLangSet`
    /// walks the bitmap word by word rather than walking the sorted table.
    pub fn langs(&self) -> impl Iterator<Item = &'static str> + 'a {
        let set = *self;
        (0..LANGS.len()).filter(move |i| set.contains_index(*i)).map(|i| LANGS[i])
    }

    /// How many languages the set holds.
    pub fn len(&self) -> usize {
        self.langs().count()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.langs().next().is_none()
    }

    /// How well this set answers a request for `lang`.
    ///
    /// This is `FcLangSetHasLang`. An exact hit is [`LangResult::Equal`];
    /// otherwise it walks outward from where `lang` would sort, taking the
    /// best result among the languages it actually has, and stops as soon as
    /// the neighbours stop being related at all.
    pub fn has_lang(&self, lang: &str) -> LangResult {
        self.has_lang_from(lang, langs::rank_of(lang))
    }

    /// The same, for a caller that already knows where `lang` sorts.
    ///
    /// Scoring asks this for every font in the set with the same tag, and
    /// the search that finds `start` is a binary search over three hundred
    /// names. It depends only on the query, so it is done once there.
    pub fn has_lang_from(&self, lang: &str, rank: std::result::Result<usize, usize>) -> LangResult {
        // Ranks, not bit indices: the walk has to go through neighbours in
        // *name* order for the early exits below to be sound.
        let start = match rank {
            Ok(rank) if self.contains_rank(rank) => return LangResult::Equal,
            Ok(rank) => rank,
            Err(insertion) => insertion,
        };

        let mut best = LangResult::DifferentLang;
        // Backwards, then forwards. Both stop at the first unrelated entry:
        // the table is sorted by name, so past that point nothing is closer.
        let mut walk = |ranks: &mut dyn Iterator<Item = usize>| {
            for rank in ranks {
                let Some(known) = langs::nth_sorted(rank) else { break };
                let result = compare_lang(lang, known);
                if result == LangResult::DifferentLang {
                    break;
                }
                if self.contains_rank(rank) && result < best {
                    best = result;
                }
            }
        };
        walk(&mut (0..start).rev());
        walk(&mut (start..LANGS.len()));
        best
    }

    /// Whether the language at alphabetical `rank` is in the set.
    fn contains_rank(&self, rank: usize) -> bool {
        langs::bit_of_rank(rank).is_some_and(|bit| self.contains_index(bit))
    }

    /// The bitmap, zero-extended to this crate's table width.
    ///
    /// A cache written by a fontconfig with a shorter map leaves the rest
    /// zero, which is what `FC_MIN (lsa->map_size, lsb->map_size)` amounts to.
    fn map(&self) -> [u32; langs::MAP_WORDS] {
        let mut map = [0; langs::MAP_WORDS];
        for (word, slot) in map.iter_mut().enumerate().take(self.map_words()) {
            *slot = self.data.u32(self.at + L.map + word * 4).unwrap_or(0);
        }
        map
    }

    /// How well this set answers another one.
    ///
    /// `FcLangSetCompare`. Any language in common is [`LangResult::Equal`];
    /// failing that, two sets naming regional variants of one language are
    /// [`LangResult::DifferentTerritory`], which is what the country sets
    /// are for.
    ///
    /// A cache never carries the extra strings the owned form can -- writing
    /// one is refused -- so this needs no equivalent of
    /// `FcLangSetCompareStrSet`.
    pub fn compare(&self, other: &LangSetRef<'_>) -> LangResult {
        compare_maps(&self.map(), &other.map())
    }

    /// Whether the bitmap fits the language table this crate was built with.
    ///
    /// A bit set past the end of [`LANGS`] means the cache was written by a
    /// fontconfig that knew more languages than we do, so every name we
    /// report for it may be shifted. The reverse -- a writer with *fewer*
    /// languages -- is undetectable, which is the larger half of the problem.
    pub fn is_consistent(&self) -> bool {
        if self.map_words() > langs::MAP_WORDS {
            return false;
        }
        (LANGS.len()..self.map_words() * 32).all(|i| !self.contains_index(i))
    }

    /// Check the bitmap is readable.
    pub fn validate(&self) -> Result<()> {
        let words = self.data.u32(self.at + L.map_size)? as usize;
        self.data.array(self.at + L.map, words, 4)?;
        Ok(())
    }
}

/// The bitmaps `FcLangSetCompare` consults when two sets share no language.
///
/// One per base language that has regional variants in [`langs::LANGS`]: every
/// `zh-*` entry in one bitmap, every `pt-*` in another, and so on. Two sets
/// with nothing in common but a bit in the same bitmap are naming regional
/// variants of one language, which scores better than being unrelated.
///
/// Derived from [`langs::LANGS`] rather than generated, because that is all
/// `fc-lang.py` does with it: group by what precedes the hyphen. It lives
/// here and not in `langs` for the same reason -- that module is generated,
/// and hand-written code in it makes the generator's own check fail.
fn country_sets() -> &'static [[u32; langs::MAP_WORDS]] {
    static SETS: std::sync::OnceLock<Vec<[u32; langs::MAP_WORDS]>> = std::sync::OnceLock::new();
    SETS.get_or_init(|| {
        let mut sets: Vec<(&str, [u32; langs::MAP_WORDS])> = Vec::new();
        for (index, lang) in LANGS.iter().enumerate() {
            let Some((base, _)) = lang.split_once('-') else { continue };
            let set = match sets.iter_mut().find(|(name, _)| *name == base) {
                Some((_, set)) => set,
                None => {
                    sets.push((base, [0; langs::MAP_WORDS]));
                    &mut sets.last_mut().expect("just pushed").1
                }
            };
            set[index / 32] |= 1 << (index % 32);
        }
        sets.into_iter().map(|(_, set)| set).collect()
    })
}

/// Every bit that appears in any country set, for a quick way out.
///
/// A language with no region is in none of them, so a set holding only such
/// languages -- which is what a query for `:lang=en` amounts to -- can be
/// answered without walking the sets at all.
fn regional_mask() -> &'static [u32; langs::MAP_WORDS] {
    static MASK: std::sync::OnceLock<[u32; langs::MAP_WORDS]> = std::sync::OnceLock::new();
    MASK.get_or_init(|| {
        let mut mask = [0; langs::MAP_WORDS];
        for set in country_sets() {
            for (slot, word) in mask.iter_mut().zip(set) {
                *slot |= word;
            }
        }
        mask
    })
}

/// The bitmap half of `FcLangSetCompare`.
///
/// One bit in common means the two name the same language. Failing that, a
/// bit each in the same country set means they name regional variants of one
/// language -- `zh-CN` against `zh-TW` -- which is a better answer than
/// unrelated, and is what stops a Simplified Chinese font scoring no closer
/// to a Traditional Chinese request than a Greek one does.
fn compare_maps(a: &[u32; langs::MAP_WORDS], b: &[u32; langs::MAP_WORDS]) -> LangResult {
    if a.iter().zip(b).any(|(a, b)| a & b != 0) {
        return LangResult::Equal;
    }
    // Nothing regional on one side means no country set can hold both, and
    // a query naming a language with no region -- `:lang=en` -- is the common
    // case. Two passes over nine words instead of ten sets of nine.
    let mask = regional_mask();
    let regional = |m: &[u32; langs::MAP_WORDS]| m.iter().zip(mask).any(|(m, k)| m & k != 0);
    if !regional(a) || !regional(b) {
        return LangResult::DifferentLang;
    }
    for set in country_sets() {
        let in_a = a.iter().zip(set).any(|(a, s)| a & s != 0);
        let in_b = b.iter().zip(set).any(|(b, s)| b & s != 0);
        if in_a && in_b {
            return LangResult::DifferentTerritory;
        }
    }
    LangResult::DifferentLang
}

/// Compare two language tags, fontconfig's `FcLangCompare`.
///
/// Tags are walked in lockstep. If they diverge where both have run out of
/// subtag, they are the same language in different regions; otherwise they
/// are unrelated. `und` -- undetermined -- never counts as equal to anything,
/// including itself.
pub fn compare_lang(a: &str, b: &str) -> LangResult {
    let undetermined = is_undetermined(a);
    let mut result = LangResult::DifferentLang;
    let mut a = a.chars();
    let mut b = b.chars();
    loop {
        let ca = a.next().map(|c| c.to_ascii_lowercase());
        let cb = b.next().map(|c| c.to_ascii_lowercase());
        match (ca, cb) {
            (None, None) => {
                return if undetermined { result } else { LangResult::Equal };
            }
            (x, y) if x != y => {
                if !undetermined && is_subtag_end(x) && is_subtag_end(y) {
                    result = LangResult::DifferentTerritory;
                }
                return result;
            }
            // Past the primary subtag, a later difference is only regional.
            (Some('-'), _) if !undetermined => {
                result = LangResult::DifferentTerritory;
            }
            _ => {}
        }
    }
}

/// Whether `super_` covers `sub`, treating a missing region as a wildcard.
///
/// `FcLangContains`. This is not symmetric with [`compare_lang`]: `en` covers
/// `en-US` *and* `en-US` covers `en`, because the side without a region is
/// taken to mean any region. Two different regions do not cover each other.
pub fn lang_contains(super_: &str, sub: &str) -> bool {
    let mut a = super_.chars();
    let mut b = sub.chars();
    loop {
        let ca = a.next().map(|c| c.to_ascii_lowercase());
        let cb = b.next().map(|c| c.to_ascii_lowercase());
        match (ca, cb) {
            (None, None) => return true,
            // One side stopped where the other starts a region.
            (Some('-'), None) | (None, Some('-')) => return true,
            (x, y) if x != y => return false,
            _ => {}
        }
    }
}

/// End of a subtag: the end of the string, or the separator.
fn is_subtag_end(c: Option<char>) -> bool {
    matches!(c, None | Some('-'))
}

fn is_undetermined(lang: &str) -> bool {
    let rest = lang.as_bytes();
    rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"und") && is_subtag_end(lang.chars().nth(3))
}

impl PartialEq for LangSetRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.langs().eq(other.langs())
    }
}

impl Eq for LangSetRef<'_> {}

impl std::fmt::Debug for LangSetRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LangSetRef({} languages)", self.len())
    }
}

/// The form `fc-list --format='%{lang}'` prints: names separated by `|`.
impl std::fmt::Display for LangSetRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, lang) in self.langs().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            f.write_str(lang)?;
        }
        Ok(())
    }
}

/// Set arithmetic, which is what a `target="scan"` rule does to a font.
#[cfg(test)]
mod set_tests {
    use super::LangSet;
    use crate::langs;

    fn langs(names: &[&str]) -> LangSet {
        let mut set = LangSet::new();
        for name in names {
            set.insert_index(langs::index_of(name).expect(name));
        }
        set
    }

    #[test]
    fn union_takes_everything_from_both() {
        let joined = langs(&["en", "ja"]).union(&langs(&["ja", "ru"]));
        assert_eq!(joined.langs().collect::<Vec<_>>(), ["en", "ja", "ru"]);
    }

    #[test]
    fn subtract_takes_only_what_the_other_has() {
        let left = langs(&["en", "hi", "ja"]).subtract(&langs(&["hi", "ru"]));
        assert_eq!(left.langs().collect::<Vec<_>>(), ["en", "ja"]);
    }

    #[test]
    fn subtracting_everything_leaves_nothing() {
        let set = langs(&["en", "ja"]);
        assert!(set.subtract(&set).is_empty());
    }

    /// A regional variant fontconfig has no bit for still has to work: a
    /// font listing `en` answers a request for `en-GB`, and the bitmap alone
    /// cannot say so.
    #[test]
    fn a_language_outside_the_table_is_kept_by_name() {
        let mut set = LangSet::new();
        set.insert("en-GB");
        assert!(langs::index_of("en-gb").is_none(), "the premise: no bit for it");
        assert_eq!(set.langs().collect::<Vec<_>>(), ["en-gb"]);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn a_broad_language_answers_a_narrower_request() {
        let font = langs(&["en"]);
        let mut asked = LangSet::new();
        asked.insert("en-GB");
        assert!(font.contains_set(&asked));
        assert!(font.contains_lang("en-GB"));
    }

    /// And the other way round, which is what `FcLangContains` means by
    /// treating a missing region as a wildcard.
    #[test]
    fn a_narrow_language_answers_a_broader_request() {
        let mut font = LangSet::new();
        font.insert("en-GB");
        assert!(font.contains_lang("en"));
        assert!(!font.contains_lang("de"));
    }

    #[test]
    fn two_different_regions_do_not_answer_each_other() {
        let mut font = LangSet::new();
        font.insert("en-GB");
        assert!(!font.contains_lang("en-US"));
    }

    #[test]
    fn names_are_folded_and_kept_once() {
        let mut set = LangSet::new();
        set.insert("EN-gb");
        set.insert("en-GB");
        assert_eq!(set.langs().collect::<Vec<_>>(), ["en-gb"]);
    }

    #[test]
    fn a_name_the_table_knows_becomes_a_bit() {
        let mut set = LangSet::new();
        set.insert("JA");
        assert_eq!(set, langs(&["ja"]));
    }

    #[test]
    fn set_arithmetic_reaches_the_names_too() {
        let mut a = LangSet::new();
        a.insert("en");
        a.insert("en-GB");
        let mut b = LangSet::new();
        b.insert("en-GB");
        assert_eq!(a.union(&b), a);
        assert_eq!(a.subtract(&b).langs().collect::<Vec<_>>(), ["en"]);
    }

    #[test]
    fn neither_operation_changes_its_operands() {
        let a = langs(&["en"]);
        let b = langs(&["ja"]);
        a.union(&b);
        a.subtract(&b);
        assert_eq!(a.langs().collect::<Vec<_>>(), ["en"]);
        assert_eq!(b.langs().collect::<Vec<_>>(), ["ja"]);
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_lang, LangResult};

    #[test]
    fn identical_tags_are_equal() {
        assert_eq!(compare_lang("en", "en"), LangResult::Equal);
        assert_eq!(compare_lang("EN", "en"), LangResult::Equal);
        assert_eq!(compare_lang("zh-cn", "zh-CN"), LangResult::Equal);
    }

    #[test]
    fn the_same_language_in_another_region_is_a_near_miss() {
        assert_eq!(compare_lang("en-US", "en-GB"), LangResult::DifferentTerritory);
        assert_eq!(compare_lang("en", "en-US"), LangResult::DifferentTerritory);
        assert_eq!(compare_lang("en-US", "en"), LangResult::DifferentTerritory);
        assert_eq!(compare_lang("zh-cn", "zh-tw"), LangResult::DifferentTerritory);
    }

    #[test]
    fn unrelated_languages_are_unrelated() {
        assert_eq!(compare_lang("en", "fr"), LangResult::DifferentLang);
        assert_eq!(compare_lang("en", "eo"), LangResult::DifferentLang);
        // A shared prefix is not a shared language: the difference falls
        // inside the primary subtag, not after it.
        assert_eq!(compare_lang("en", "eng"), LangResult::DifferentLang);
    }

    /// `und` means the language was not determined, so it is never equal to
    /// anything -- not even to itself.
    #[test]
    fn undetermined_is_never_equal() {
        assert_eq!(compare_lang("und", "und"), LangResult::DifferentLang);
        assert_eq!(compare_lang("und", "en"), LangResult::DifferentLang);
        assert_eq!(compare_lang("und-zsye", "und-zsye"), LangResult::DifferentLang);
        // Only the exact tag "und" is special; "undo" is an ordinary string.
        assert_eq!(compare_lang("undo", "undo"), LangResult::Equal);
    }

    #[test]
    fn results_order_from_best_to_worst() {
        assert!(LangResult::Equal < LangResult::DifferentTerritory);
        assert!(LangResult::DifferentTerritory < LangResult::DifferentLang);
    }
}

/// The four languages an `OS/2` codepage bit can single out.
///
/// `FcCodePageRange` in `fclang.c`, as bits of `ulCodePageRange1`. Only these
/// take part in the exclusivity rule; a font declaring Simplified Chinese
/// still gets `zh-sg` from its coverage, because `zh-sg` is not one of them.
pub(crate) const CODE_PAGES: [(u32, &str); 4] =
    [(17, "ja"), (18, "zh-cn"), (19, "ko"), (20, "zh-tw")];

/// The language a font declares, if it declares exactly one.
///
/// Two or more means the font supports several and none is exclusive, which
/// fontconfig treats the same as declaring nothing.
#[cfg(feature = "scan")]
pub(crate) fn exclusive_from_code_pages(range1: u32) -> Option<usize> {
    let mut found = None;
    for (bit, name) in CODE_PAGES {
        if range1 & (1 << bit) == 0 {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = crate::langs::index_of(name);
    }
    found
}

/// Whether language `index` is one the codepage bits can name.
fn is_exclusive_lang(index: usize) -> bool {
    CODE_PAGES.iter().any(|(_, name)| crate::langs::index_of(name) == Some(index))
}

/// How many 256-codepoint pages a language's orthography touches.
///
/// Fontconfig holds each orthography as a charset and compares `charset.num`,
/// the number of leaves; a leaf is a page, so this is the same number counted
/// from the ranges the table stores instead.
fn orthography_pages(index: usize) -> usize {
    let mut pages = 0usize;
    let mut last: Option<u32> = None;
    for (lo, hi) in crate::orth::orthography(index) {
        for page in (lo / 256)..=(hi / 256) {
            if last != Some(page) {
                pages += 1;
                last = Some(page);
            }
        }
    }
    pages
}

/// A set of languages built by scanning a font, rather than read from a cache.
///
/// Same bitmap, same bit order; the difference is only where the bytes live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LangSet {
    bits: [u32; langs::MAP_WORDS],
    /// Languages fontconfig's table cannot name.
    ///
    /// `FcLangSet` keeps these in a string set of its own, and it has to:
    /// the bitmap can only say what the table names, and `en-GB` is not one
    /// of those even though `en` is. Dropping them would make a selector or
    /// a query for a regional variant match nothing at all.
    ///
    /// Never serialized -- fontconfig rejects a cache whose language set has
    /// one -- so in practice only a query or a configuration ever carries
    /// any. Kept sorted so two sets compare by value.
    extra: Vec<String>,
}

impl LangSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Which languages a font covering `coverage` can write.
    ///
    /// A language is included when the font covers *every* codepoint of its
    /// orthography. There is no partial credit and no threshold: that one
    /// rule is the whole of `FcLangSetFromCharSet`.
    pub fn from_char_set(chars: &crate::charset::CharSet) -> Self {
        Self::from_char_set_exclusive(chars, None)
    }

    /// The languages `chars` can write, with the Han rule applied.
    ///
    /// A font that declares exactly one CJK codepage in `OS/2` is taken to
    /// mean it: the other Han languages are not derived from its coverage
    /// however much of their orthographies it happens to carry. Microsoft
    /// YaHei declares Simplified Chinese and covers enough of Japanese and
    /// Traditional Chinese to satisfy both, and fontconfig reports neither.
    ///
    /// `exclusive` is an index into [`LANGS`](crate::langs::LANGS). A font
    /// declaring several codepages, or none, passes `None` and every language
    /// is derived from coverage alone -- which is what fontconfig does when
    /// the font cannot make up its mind.
    pub fn from_char_set_exclusive(
        chars: &crate::charset::CharSet,
        exclusive: Option<usize>,
    ) -> Self {
        let mut set = Self::new();
        // Fontconfig compares the candidate's orthography against the
        // declared one by page count, so a Han language sized differently
        // from the declared one is skipped. It is a coarse test and an
        // intentional one: the four are far enough apart in size that it
        // separates them, and a language of the same size is one the font
        // could equally be said to write.
        let declared_pages = exclusive.map(orthography_pages);
        for index in 0..crate::orth::len() {
            if let Some(pages) = declared_pages {
                if is_exclusive_lang(index) && orthography_pages(index) != pages {
                    continue;
                }
            }
            if chars.covers_ranges(crate::orth::orthography(index)) {
                set.insert_index(index);
            }
        }
        set
    }

    /// Mark language `index` as supported.
    pub fn insert_index(&mut self, index: usize) {
        if let Some(word) = self.bits.get_mut(index / 32) {
            *word |= 1 << (index % 32);
        }
    }

    /// Add a language by name.
    ///
    /// A name the table knows sets a bit; anything else is kept as a string.
    /// Names are compared without case, so they are stored folded.
    pub fn insert(&mut self, lang: &str) {
        let lang = lang.to_lowercase();
        match langs::index_of(&lang) {
            Some(index) => self.insert_index(index),
            None => {
                if let Err(at) = self.extra.binary_search(&lang) {
                    self.extra.insert(at, lang);
                }
            }
        }
    }

    /// Whether bit `index` is set.
    ///
    /// Only the table half: see [`LangSet::contains_lang`] for the question
    /// that also consults the names the table cannot hold.
    pub fn contains_index(&self, index: usize) -> bool {
        self.bits.get(index / 32).is_some_and(|word| word & (1 << (index % 32)) != 0)
    }

    /// Every language in the set: the table half in bit order, then any
    /// name the table could not hold.
    pub fn langs(&self) -> impl Iterator<Item = &str> + '_ {
        (0..LANGS.len())
            .filter(|i| self.contains_index(*i))
            .map(|i| LANGS[i])
            .chain(self.extra.iter().map(String::as_str))
    }

    /// Every language in the set, however it is stored.
    ///
    /// `FcLangSetCopy` copies the extra names as well as the bitmap, and so
    /// does this: a set holding only `en-GB` -- a name the table cannot
    /// express, so it is a string and nothing else -- would otherwise come
    /// back empty.
    pub fn from_languages(languages: &AnyLangSet<'_>) -> Self {
        let mut set = Self::new();
        for index in 0..LANGS.len() {
            if languages.contains_index(index) {
                set.insert_index(index);
            }
        }
        set.extra.extend(languages.extra().iter().cloned());
        set
    }

    /// Everything in either set.
    pub fn union(&self, other: &Self) -> Self {
        let mut out = self.clone();
        for (word, bits) in out.bits.iter_mut().zip(other.bits.iter()) {
            *word |= bits;
        }
        for name in &other.extra {
            if let Err(at) = out.extra.binary_search(name) {
                out.extra.insert(at, name.clone());
            }
        }
        out
    }

    /// Everything in this set that is not in `other`.
    ///
    /// This is what a `target="scan"` rule uses to take a language away from
    /// a font that covers its characters without really supporting it: the
    /// GNU FreeFont faces claim every Devanagari codepoint and render none of
    /// the conjuncts, so the config subtracts `hi`, `mr`, `sa` and the rest.
    pub fn subtract(&self, other: &Self) -> Self {
        let mut out = self.clone();
        for (word, bits) in out.bits.iter_mut().zip(other.bits.iter()) {
            *word &= !bits;
        }
        out.extra.retain(|name| !other.extra.contains(name));
        out
    }

    /// Whether this set answers everything `other` asks for.
    ///
    /// `FcLangSetContains`, which is what a `<patelt>` comparison uses. A
    /// language missing outright is still covered when the set holds one that
    /// contains it, so a font listing `en` satisfies a request for `en-US`.
    pub fn contains_set(&self, other: &Self) -> bool {
        (0..LANGS.len())
            .filter(|index| other.contains_index(*index) && !self.contains_index(*index))
            .all(|index| self.contains_lang(LANGS[index]))
            && other.extra.iter().all(|name| self.contains_lang(name))
    }

    /// Whether the set holds `lang` or something that covers it.
    ///
    /// `FcLangSetContainsLang`. The walk goes outward from where `lang` would
    /// sort and stops as soon as the neighbours are a different language,
    /// which is why the table has to be searched in name order rather than
    /// bit order.
    pub fn contains_lang(&self, lang: &str) -> bool {
        if self.extra.iter().any(|name| lang_contains(name, lang)) {
            return true;
        }
        let start = match langs::rank_of(lang) {
            Ok(rank) if self.contains_rank(rank) => return true,
            Ok(rank) => rank,
            Err(insertion) => insertion,
        };
        let walk = |ranks: &mut dyn Iterator<Item = usize>| {
            for rank in ranks {
                let Some(known) = langs::nth_sorted(rank) else { break };
                if compare_lang(known, lang) == LangResult::DifferentLang {
                    break;
                }
                if self.contains_rank(rank) && lang_contains(known, lang) {
                    return true;
                }
            }
            false
        };
        walk(&mut (start..LANGS.len())) || walk(&mut (0..start).rev())
    }

    fn contains_rank(&self, rank: usize) -> bool {
        langs::nth_sorted(rank)
            .and_then(langs::index_of)
            .is_some_and(|index| self.contains_index(index))
    }

    /// The raw bitmap, as the cache stores it.
    pub(crate) fn words(&self) -> &[u32; langs::MAP_WORDS] {
        &self.bits
    }

    /// How many languages the set holds.
    pub fn len(&self) -> usize {
        let bits: usize = self.bits.iter().map(|w| w.count_ones() as usize).sum();
        bits + self.extra.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A reference to a set of languages, whichever way it is stored.
///
/// The language counterpart to [`AnyCharSet`](crate::AnyCharSet), and the
/// same three-way split: [`LangSetRef`] borrows a cache's bitmap,
/// [`LangSet`] holds its own and can grow, and this is either of them
/// seen through a reference. Both arms are borrows, so this stays `Copy`.
#[derive(Clone, Copy, Debug)]
pub enum AnyLangSet<'a> {
    /// Read from a cache.
    Cached(LangSetRef<'a>),
    /// Built by scanning a font.
    Owned(&'a LangSet),
}

impl<'a> AnyLangSet<'a> {
    /// Whether bit `index` is set.
    pub fn contains_index(&self, index: usize) -> bool {
        match self {
            Self::Cached(set) => set.contains_index(index),
            Self::Owned(set) => set.contains_index(index),
        }
    }

    /// Whether the font covers exactly this language, by name.
    ///
    /// A name the table cannot express is still in the set if the scanner put
    /// it there, so the extra names are searched too -- case-insensitively,
    /// which is how `FcStrCmpIgnoreCase` compares language names everywhere
    /// else.
    pub fn contains(&self, lang: &str) -> bool {
        if langs::index_of(lang).is_some_and(|i| self.contains_index(i)) {
            return true;
        }
        self.extra().iter().any(|name| name.eq_ignore_ascii_case(lang))
    }

    /// Every language in the set: the table half in bit order, then any name
    /// the table could not hold.
    ///
    /// The second half is always empty for a cached set. Upstream does not
    /// serialize it -- `FcLangSetSerialize` sets `extra` to `NULL` and says
    /// why -- so only a set built by scanning can carry one.
    pub fn langs(self) -> impl Iterator<Item = &'a str> {
        let extra: &'a [String] = match self {
            Self::Cached(_) => &[],
            Self::Owned(set) => &set.extra,
        };
        (0..LANGS.len())
            .filter(move |i| self.contains_index(*i))
            .map(|i| LANGS[i])
            .chain(extra.iter().map(String::as_str))
    }

    /// How many languages the set holds.
    pub fn len(&self) -> usize {
        (0..LANGS.len()).filter(|i| self.contains_index(*i)).count() + self.extra().len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How well this set answers a request for `lang`.
    pub fn has_lang(&self, lang: &str) -> LangResult {
        self.has_lang_from(lang, langs::rank_of(lang))
    }

    /// The same, for a caller that already knows where `lang` sorts.
    pub fn has_lang_from(&self, lang: &str, rank: std::result::Result<usize, usize>) -> LangResult {
        // Ranks, not bit indices: the walk goes through neighbours in name
        // order for the early exits to be sound.
        let start = match rank {
            Ok(rank) if self.contains_rank(rank) => return LangResult::Equal,
            Ok(rank) => rank,
            Err(insertion) => insertion,
        };
        let mut best = LangResult::DifferentLang;
        let mut walk = |ranks: &mut dyn Iterator<Item = usize>| {
            for rank in ranks {
                let Some(known) = langs::nth_sorted(rank) else { break };
                let result = compare_lang(lang, known);
                if result == LangResult::DifferentLang {
                    break;
                }
                if self.contains_rank(rank) && result < best {
                    best = result;
                }
            }
        };
        walk(&mut (0..start).rev());
        walk(&mut (start..LANGS.len()));
        // `FcLangSetHasLang` finishes with the languages the table cannot
        // name. Without this a set holding only `en-GB` -- which is not in the
        // table, so it is a string and nothing else -- answers "unrelated" to
        // every request, including one for `en-GB` itself.
        for name in self.extra() {
            if best == LangResult::Equal {
                break;
            }
            let result = compare_lang(lang, name);
            if result < best {
                best = result;
            }
        }
        best
    }

    fn contains_rank(&self, rank: usize) -> bool {
        langs::bit_of_rank(rank).is_some_and(|bit| self.contains_index(bit))
    }

    /// How well this set answers another one.
    ///
    /// `FcLangSetCompare`: a language in common is [`LangResult::Equal`],
    /// regional variants of one language are
    /// [`LangResult::DifferentTerritory`], and the extra strings an owned set
    /// carries get a pass of their own, in both directions.
    pub fn compare(&self, other: &AnyLangSet<'_>) -> LangResult {
        let mut best = compare_maps(&self.map(), &other.map());
        if best == LangResult::Equal {
            return best;
        }
        // `FcLangSetCompareStrSet` both ways round, stopping as soon as
        // nothing left can improve on what we have.
        for (set, extra) in [(other, self.extra()), (self, other.extra())] {
            for name in extra {
                let r = set.has_lang(name);
                if r < best {
                    best = r;
                }
                if best == LangResult::Equal {
                    return best;
                }
            }
        }
        best
    }

    /// The bitmap, zero-extended to this crate's table width.
    fn map(&self) -> [u32; langs::MAP_WORDS] {
        match self {
            Self::Cached(set) => set.map(),
            Self::Owned(set) => set.bits,
        }
    }

    /// The languages held as strings because the table cannot name them.
    fn extra(&self) -> &[String] {
        match self {
            Self::Cached(_) => &[],
            Self::Owned(set) => &set.extra,
        }
    }

    /// Check the structure, for a set that has one to check.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Cached(set) => set.validate(),
            Self::Owned(_) => Ok(()),
        }
    }

    /// Whether the bitmap fits the language table this crate was built with.
    pub fn is_consistent(&self) -> bool {
        match self {
            Self::Cached(set) => set.is_consistent(),
            Self::Owned(_) => true,
        }
    }
}

/// `FcLangSetEqual`: the bitmaps, and then the extra names as a set.
impl PartialEq for AnyLangSet<'_> {
    fn eq(&self, other: &Self) -> bool {
        if !(0..LANGS.len()).all(|i| self.contains_index(i) == other.contains_index(i)) {
            return false;
        }
        // A set, not a list: `FcStrSetEqual` compares membership, and the
        // scanner does not promise an order. A cached set never has any --
        // `FcLangSetSerialize` says so in as many words -- so this only ever
        // separates two owned sets.
        let (ours, theirs) = (self.extra(), other.extra());
        ours.len() == theirs.len() && ours.iter().all(|name| theirs.contains(name))
    }
}

impl Eq for AnyLangSet<'_> {}

/// The form `fc-list --format='%{lang}'` prints: names separated by `|`.
impl std::fmt::Display for AnyLangSet<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, lang) in self.langs().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            f.write_str(lang)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod country_set_tests {
    use super::{compare_maps, AnyLangSet, LangResult, LangSet};
    use crate::langs;

    fn set(names: &[&str]) -> LangSet {
        let mut s = LangSet::new();
        for n in names {
            s.insert(n);
        }
        s
    }

    fn cmp(a: &[&str], b: &[&str]) -> LangResult {
        AnyLangSet::Owned(&set(a)).compare(&AnyLangSet::Owned(&set(b)))
    }

    /// `FcLangSetCompare` looks past "no language in common" to whether the
    /// two name regional variants of one language, which scores better than
    /// unrelated. Without it a Simplified Chinese font is no closer to a
    /// Traditional Chinese request than a Greek one.
    #[test]
    fn regional_variants_of_one_language_are_not_unrelated() {
        assert_eq!(cmp(&["zh-cn"], &["zh-cn"]), LangResult::Equal);
        assert_eq!(cmp(&["zh-cn"], &["zh-tw"]), LangResult::DifferentTerritory);
        assert_eq!(cmp(&["zh-cn"], &["el"]), LangResult::DifferentLang);
        assert_eq!(cmp(&["en"], &["de"]), LangResult::DifferentLang);
    }

    /// The extra strings an owned set carries get their own pass, in both
    /// directions.
    ///
    /// `en-GB` is not in the language table, so it lands among the extras and
    /// only `FcLangSetCompareStrSet` can see it at all. The answer is
    /// `DifferentTerritory` rather than `Equal`: `FcLangSetHasLang` reaches
    /// `en` through `FcLangCompare`, which calls the same language in a
    /// different region exactly that. Reaching `Equal` here was the obvious
    /// guess and the wrong one.
    #[test]
    fn the_extra_strings_are_compared_too() {
        assert_eq!(cmp(&["en-gb"], &["en"]), LangResult::DifferentTerritory);
        assert_eq!(cmp(&["en"], &["en-gb"]), LangResult::DifferentTerritory);
        assert_eq!(cmp(&["en-gb"], &["de"]), LangResult::DifferentLang);
        // Two extras naming the same thing still reach Equal.
        assert_eq!(cmp(&["en-gb"], &["en-gb"]), LangResult::Equal);
    }

    /// Every set groups languages that share a base, and a language with no
    /// region belongs to none of them.
    #[test]
    fn the_country_sets_group_by_base_language() {
        let sets = super::country_sets();
        assert!(!sets.is_empty(), "the table has regional variants in it");
        let bit = |lang: &str| {
            let index = langs::LANGS.iter().position(|l| *l == lang)?;
            Some((index / 32, 1u32 << (index % 32)))
        };
        let (word, mask) = bit("zh-cn").expect("zh-cn is in the table");
        let holding = sets.iter().filter(|s| s[word] & mask != 0).count();
        assert_eq!(holding, 1, "a language belongs to exactly one country set");

        // A language with no region is in none of them.
        if let Some((word, mask)) = bit("el") {
            assert!(sets.iter().all(|s| s[word] & mask == 0));
        }
    }

    #[test]
    fn an_empty_pair_is_unrelated_rather_than_equal() {
        let zero = [0; langs::MAP_WORDS];
        assert_eq!(compare_maps(&zero, &zero), LangResult::DifferentLang);
    }
}
