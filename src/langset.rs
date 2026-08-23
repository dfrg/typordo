//! The languages a font can write.
//!
//! Fontconfig decides this by checking a font's coverage against an
//! orthography per language, and stores the answer as a bitmap over its own
//! language list. See [`langs`](crate::langs) for why that list is an
//! assumption about whichever fontconfig wrote the cache.

use crate::bytes::Bytes;
use crate::error::Result;
use crate::langs::{self, LANGS};

/// `FcLangSet` is `extra` (8, never serialized), `map_size` (4), `map[]`.
const MAP_SIZE: usize = 8;
const MAP: usize = 12;

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

/// The set of languages a font supports.
#[derive(Clone, Copy)]
pub struct LangSet<'a> {
    pub(crate) data: Bytes<'a>,
    pub(crate) at: usize,
}

impl<'a> LangSet<'a> {
    /// How many 32-bit words the stored bitmap has.
    ///
    /// Compare against [`langs::MAP_WORDS`] to see whether the writer sized
    /// its language list the same way we do.
    pub fn map_words(&self) -> usize {
        self.data.u32(self.at + MAP_SIZE).unwrap_or(0) as usize
    }

    /// Whether bit `index` is set.
    pub fn contains_index(&self, index: usize) -> bool {
        let word = index / 32;
        if word >= self.map_words() {
            return false;
        }
        let Ok(bits) = self.data.u32(self.at + MAP + word * 4) else {
            return false;
        };
        bits & (1 << (index % 32)) != 0
    }

    /// Whether the font covers exactly this language, by name.
    ///
    /// This is an exact table lookup. Use [`LangSet::has_lang`] to ask the
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
        // Ranks, not bit indices: the walk has to go through neighbours in
        // *name* order for the early exits below to be sound.
        let start = match langs::rank_of(lang) {
            Ok(rank) if self.contains_rank(rank) => return LangResult::Equal,
            Ok(rank) => rank,
            // Not in the table: start from where it would have been.
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

    /// How well this set answers another one.
    ///
    /// Any language in common is [`LangResult::Equal`]. Fontconfig then has a
    /// second pass over its generated country sets, which can turn two
    /// disjoint sets into [`LangResult::DifferentTerritory`] when they share
    /// a country; that table is not embedded here, so this reports
    /// [`LangResult::DifferentLang`] in that case instead. A query built with
    /// [`Query`](crate::Query) cannot carry a langset, so this is only
    /// reachable when comparing two fonts.
    pub fn compare(&self, other: &LangSet<'_>) -> LangResult {
        let words = self.map_words().min(other.map_words()).min(langs::MAP_WORDS);
        for word in 0..words {
            let ours = self.data.u32(self.at + MAP + word * 4).unwrap_or(0);
            let theirs = other.data.u32(other.at + MAP + word * 4).unwrap_or(0);
            if ours & theirs != 0 {
                return LangResult::Equal;
            }
        }
        LangResult::DifferentLang
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
        let words = self.data.u32(self.at + MAP_SIZE)? as usize;
        self.data.array(self.at + MAP, words, 4)?;
        Ok(())
    }
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

/// End of a subtag: the end of the string, or the separator.
fn is_subtag_end(c: Option<char>) -> bool {
    matches!(c, None | Some('-'))
}

fn is_undetermined(lang: &str) -> bool {
    let rest = lang.as_bytes();
    rest.len() >= 3
        && rest[..3].eq_ignore_ascii_case(b"und")
        && is_subtag_end(lang.chars().nth(3))
}

impl PartialEq for LangSet<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.langs().eq(other.langs())
    }
}

impl Eq for LangSet<'_> {}

impl std::fmt::Debug for LangSet<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LangSet({} languages)", self.len())
    }
}

/// The form `fc-list --format='%{lang}'` prints: names separated by `|`.
impl std::fmt::Display for LangSet<'_> {
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
    use super::Langs;
    use crate::langs;

    fn langs(names: &[&str]) -> Langs {
        let mut set = Langs::new();
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

/// A set of languages built by scanning a font, rather than read from a cache.
///
/// Same bitmap, same bit order; the difference is only where the bytes live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Langs {
    bits: [u32; langs::MAP_WORDS],
}

impl Langs {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Which languages a font covering `coverage` can write.
    ///
    /// A language is included when the font covers *every* codepoint of its
    /// orthography. There is no partial credit and no threshold: that one
    /// rule is the whole of `FcLangSetFromCharSet`.
    pub fn from_coverage(coverage: &crate::charset::Coverage) -> Self {
        let mut set = Self::new();
        for index in 0..crate::orth::len() {
            if coverage.covers_ranges(crate::orth::orthography(index)) {
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

    /// Whether bit `index` is set.
    pub fn contains_index(&self, index: usize) -> bool {
        self.bits
            .get(index / 32)
            .is_some_and(|word| word & (1 << (index % 32)) != 0)
    }

    /// Every language in the set, in bit order.
    pub fn langs(&self) -> impl Iterator<Item = &'static str> + '_ {
        (0..LANGS.len()).filter(|i| self.contains_index(*i)).map(|i| LANGS[i])
    }

    /// Every language in the set, however it is stored.
    pub fn from_languages(languages: &Languages<'_>) -> Self {
        let mut set = Self::new();
        for index in 0..LANGS.len() {
            if languages.contains_index(index) {
                set.insert_index(index);
            }
        }
        set
    }

    /// Everything in either set.
    pub fn union(&self, other: &Self) -> Self {
        let mut out = self.clone();
        for (word, bits) in out.bits.iter_mut().zip(other.bits.iter()) {
            *word |= bits;
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
        out
    }

    /// The raw bitmap, as the cache stores it.
    pub(crate) fn words(&self) -> &[u32; langs::MAP_WORDS] {
        &self.bits
    }

    /// How many languages the set holds.
    pub fn len(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A set of languages, however it happens to be stored.
#[derive(Clone, Copy, Debug)]
pub enum Languages<'a> {
    /// Read from a cache.
    Cached(LangSet<'a>),
    /// Built by scanning a font.
    Owned(&'a Langs),
}

impl<'a> Languages<'a> {
    /// Whether bit `index` is set.
    pub fn contains_index(&self, index: usize) -> bool {
        match self {
            Self::Cached(set) => set.contains_index(index),
            Self::Owned(set) => set.contains_index(index),
        }
    }

    /// Whether the font covers exactly this language, by name.
    pub fn contains(&self, lang: &str) -> bool {
        langs::index_of(lang).is_some_and(|i| self.contains_index(i))
    }

    /// Every language in the set, in bit order.
    pub fn langs(self) -> impl Iterator<Item = &'static str> + 'a {
        (0..LANGS.len()).filter(move |i| self.contains_index(*i)).map(|i| LANGS[i])
    }

    /// How many languages the set holds.
    pub fn len(&self) -> usize {
        (0..LANGS.len()).filter(|i| self.contains_index(*i)).count()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How well this set answers a request for `lang`.
    pub fn has_lang(&self, lang: &str) -> LangResult {
        // Ranks, not bit indices: the walk goes through neighbours in name
        // order for the early exits to be sound.
        let start = match langs::rank_of(lang) {
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
        best
    }

    fn contains_rank(&self, rank: usize) -> bool {
        langs::bit_of_rank(rank).is_some_and(|bit| self.contains_index(bit))
    }

    /// How well this set answers another one.
    pub fn compare(&self, other: &Languages<'_>) -> LangResult {
        for index in 0..LANGS.len() {
            if self.contains_index(index) && other.contains_index(index) {
                return LangResult::Equal;
            }
        }
        LangResult::DifferentLang
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

impl PartialEq for Languages<'_> {
    fn eq(&self, other: &Self) -> bool {
        (0..LANGS.len()).all(|i| self.contains_index(i) == other.contains_index(i))
    }
}

impl Eq for Languages<'_> {}

/// The form `fc-list --format='%{lang}'` prints: names separated by `|`.
impl std::fmt::Display for Languages<'_> {
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
