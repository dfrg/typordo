//! The set of characters a font can draw.
//!
//! Fontconfig stores coverage as a sparse two-level bitmap: an ascending list
//! of 256-codepoint pages, and one 256-bit leaf per page. A font covering
//! Latin-1 and nothing else carries two leaves, not a bitmap over all of
//! Unicode.

use crate::bytes::Bytes;
use crate::error::{Error, Result};

/// `FcCharSet` is `ref` (4), `num` (4), `leaves_offset` (8), `numbers_offset` (8).
const NUM: usize = 4;
const LEAVES: usize = 8;
const NUMBERS: usize = 16;

/// A leaf covers 256 codepoints as eight 32-bit words.
const LEAF_WORDS: usize = 8;
const LEAF_BYTES: usize = LEAF_WORDS * 4;
/// Codepoints per page.
pub(crate) const PAGE: u32 = 256;

/// The characters a font covers.
///
/// Borrowed from the cache like everything else; no coverage data is copied.
#[derive(Clone, Copy)]
pub struct CharSet<'a> {
    pub(crate) data: Bytes<'a>,
    pub(crate) at: usize,
}

impl<'a> CharSet<'a> {
    /// How many pages of coverage this set holds.
    ///
    /// Each page is up to 256 codepoints; see [`CharSet::len`] for the number
    /// of characters actually covered.
    pub fn pages(&self) -> usize {
        self.checked_pages().unwrap_or(0)
    }

    /// True when the set covers no characters at all.
    pub fn is_empty(&self) -> bool {
        self.pages() == 0
    }

    /// Whether `c` is covered.
    ///
    /// Pages are stored in ascending order, so this is a binary search over
    /// them and then one bit test.
    pub fn contains(&self, c: char) -> bool {
        self.contains_u32(c as u32)
    }

    fn contains_u32(&self, c: u32) -> bool {
        let Some(page) = u16::try_from(c / PAGE).ok() else {
            return false;
        };
        let Some(index) = self.find_page(page) else {
            return false;
        };
        let Ok(word) = self.leaf_word(index, ((c % PAGE) / 32) as usize) else {
            return false;
        };
        word & (1 << (c % 32)) != 0
    }

    /// How many characters the set covers.
    pub fn len(&self) -> usize {
        let mut total = 0;
        for index in 0..self.pages() {
            for word in 0..LEAF_WORDS {
                total += self.leaf_word(index, word).unwrap_or(0).count_ones() as usize;
            }
        }
        total
    }

    /// The covered characters, ascending.
    pub fn chars(&self) -> impl Iterator<Item = char> + 'a {
        let set = *self;
        (0..set.pages()).flat_map(move |index| {
            let base = u32::from(set.page_number(index).unwrap_or(0)) * PAGE;
            (0..LEAF_WORDS).flat_map(move |word| {
                let bits = set.leaf_word(index, word).unwrap_or(0);
                (0..32u32)
                    .filter(move |bit| bits & (1 << bit) != 0)
                    .filter_map(move |bit| char::from_u32(base + word as u32 * 32 + bit))
            })
        })
    }

    /// The covered characters as contiguous inclusive ranges, ascending.
    ///
    /// This is the shape `fc-query` prints a charset in, and far cheaper to
    /// compare than the expanded character list.
    pub fn ranges(&self) -> impl Iterator<Item = (char, char)> + 'a {
        let mut chars = self.chars();
        let mut pending = chars.next();
        std::iter::from_fn(move || {
            let start = pending?;
            let mut end = start;
            loop {
                match chars.next() {
                    Some(next) if next as u32 == end as u32 + 1 => end = next,
                    other => {
                        pending = other;
                        return Some((start, end));
                    }
                }
            }
        })
    }

    /// Walk the whole structure, reporting the first problem.
    pub fn validate(&self) -> Result<()> {
        let pages = self.checked_pages()?;
        let numbers = self.numbers_base()?;
        self.data.array(numbers, pages, 2)?;
        let leaves = self.leaves_base()?;
        self.data.array(leaves, pages, 8)?;
        let mut previous = None;
        for index in 0..pages {
            let page = self.page_number(index)?;
            // Ascending order is what makes the binary search in `contains`
            // correct, so it is a structural requirement, not a nicety.
            if previous.is_some_and(|p| p >= page) {
                return Err(Error::BadCount(i32::from(page)));
            }
            previous = Some(page);
            for word in 0..LEAF_WORDS {
                self.leaf_word(index, word)?;
            }
        }
        Ok(())
    }

    fn checked_pages(&self) -> Result<usize> {
        self.data.count(self.at + NUM)
    }

    fn numbers_base(&self) -> Result<usize> {
        self.data.resolve(self.at, self.data.i64(self.at + NUMBERS)?)
    }

    fn leaves_base(&self) -> Result<usize> {
        self.data.resolve(self.at, self.data.i64(self.at + LEAVES)?)
    }

    /// The page number stored at `index`.
    pub(crate) fn page_number(&self, index: usize) -> Result<u16> {
        let at = self.numbers_base()? + index * 2;
        Ok(u16::try_from(self.data.u32(at)? & 0xffff).unwrap_or(0))
    }

    /// One 32-bit word of leaf `index`.
    ///
    /// Leaf offsets are relative to the start of the leaf array, the same
    /// convention the cache's subdirectory list uses.
    pub(crate) fn leaf_word(&self, index: usize, word: usize) -> Result<u32> {
        let leaves = self.leaves_base()?;
        let delta = self.data.i64(leaves + index * 8)?;
        let leaf = self.data.resolve(leaves, delta)?;
        if word >= LEAF_WORDS {
            return Err(Error::Truncated { at: leaf, len: LEAF_BYTES });
        }
        self.data.u32(leaf + word * 4)
    }

    fn find_page(&self, page: u16) -> Option<usize> {
        let pages = self.checked_pages().ok()?;
        let (mut low, mut high) = (0usize, pages);
        while low < high {
            let mid = (low + high) / 2;
            match self.page_number(mid).ok()?.cmp(&page) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }
}

impl PartialEq for CharSet<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.pages() == other.pages() && self.chars().eq(other.chars())
    }
}

impl Eq for CharSet<'_> {}

impl std::fmt::Debug for CharSet<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CharSet({} chars in {} pages)", self.len(), self.pages())
    }
}

/// Format a charset the way `fc-query --format='%{charset}'` does: inclusive
/// hex ranges, low-to-high, space separated, with a single character written
/// once rather than as `x-x`.
impl std::fmt::Display for CharSet<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (start, end)) in self.ranges().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            if start == end {
                write!(f, "{:x}", start as u32)?;
            } else {
                write!(f, "{:x}-{:x}", start as u32, end as u32)?;
            }
        }
        Ok(())
    }
}

/// A growable union of character sets.
///
/// Sorting a font list needs to know whether each font adds anything the ones
/// before it did not, which means accumulating coverage as the walk proceeds.
/// The layout mirrors [`CharSet`]'s -- 256-codepoint pages of eight words --
/// so merging a font is a handful of word-ORs per page rather than a pass over
/// its characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pages: std::collections::HashMap<u16, [u32; LEAF_WORDS]>,
}

impl Coverage {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add everything in a set of characters, however it is stored.
    ///
    /// The bool is the whole point: it is what decides that a font earns its
    /// place in a fallback list.
    pub fn merge_chars(&mut self, other: &Chars<'_>) -> bool {
        match other {
            Chars::Cached(set) => self.merge(set),
            Chars::Owned(set) => {
                let mut added = false;
                for c in set.chars() {
                    if !self.contains(c) {
                        added = true;
                        self.insert(c);
                    }
                }
                added
            }
        }
    }

    /// Add everything in `other`, reporting whether it contributed anything.
    ///
    /// The bool is the whole point: it is what decides that a font earns its
    /// place in a fallback list.
    pub fn merge(&mut self, other: &CharSet<'_>) -> bool {
        let mut added = false;
        for index in 0..other.pages() {
            let Ok(page) = other.page_number(index) else { continue };
            let leaf = self.pages.entry(page).or_insert([0; LEAF_WORDS]);
            for (word, slot) in leaf.iter_mut().enumerate() {
                let Ok(bits) = other.leaf_word(index, word) else { continue };
                // Anything set there that was not set here is new.
                if bits & !*slot != 0 {
                    added = true;
                }
                *slot |= bits;
            }
        }
        added
    }

    /// Whether `c` is covered.
    pub fn contains(&self, c: char) -> bool {
        let page = (c as u32 / PAGE) as u16;
        self.pages.get(&page).is_some_and(|leaf| {
            leaf[((c as u32 % PAGE) / 32) as usize] & (1 << (c as u32 % 32)) != 0
        })
    }

    /// How many characters the union holds.
    pub fn len(&self) -> usize {
        self.pages
            .values()
            .map(|leaf| leaf.iter().map(|w| w.count_ones() as usize).sum::<usize>())
            .sum()
    }

    /// Whether nothing has been merged in.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add one character.
    pub fn insert(&mut self, c: char) {
        let page = (c as u32 / PAGE) as u16;
        let leaf = self.pages.entry(page).or_insert([0; LEAF_WORDS]);
        leaf[((c as u32 % PAGE) / 32) as usize] |= 1 << (c as u32 % 32);
    }

    /// The covered characters, ascending.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        let mut pages: Vec<u16> = self.pages.keys().copied().collect();
        pages.sort_unstable();
        pages.into_iter().flat_map(move |page| {
            let leaf = self.pages[&page];
            let base = u32::from(page) * PAGE;
            (0..LEAF_WORDS).flat_map(move |word| {
                let bits = leaf[word];
                (0..32u32)
                    .filter(move |bit| bits & (1 << bit) != 0)
                    .filter_map(move |bit| char::from_u32(base + word as u32 * 32 + bit))
            })
        })
    }

    /// Whether every character in `ranges` is covered.
    ///
    /// This is the question a language orthography asks.
    pub fn covers_ranges(&self, ranges: &[(u32, u32)]) -> bool {
        ranges.iter().all(|(lo, hi)| {
            (*lo..=*hi).all(|c| char::from_u32(c).is_none_or(|c| self.contains(c)))
        })
    }
}

/// A set of characters, however it happens to be stored.
///
/// Coverage read from a cache borrows its bytes; coverage produced by
/// scanning a font is built in memory. Both answer the same questions, so
/// matching and reporting take this rather than one or the other.
#[derive(Clone, Copy, Debug)]
pub enum Chars<'a> {
    /// Read from a cache.
    Cached(CharSet<'a>),
    /// Built by scanning a font.
    Owned(&'a Coverage),
}

impl<'a> Chars<'a> {
    /// Whether `c` is covered.
    pub fn contains(&self, c: char) -> bool {
        match self {
            Self::Cached(set) => set.contains(c),
            Self::Owned(set) => set.contains(c),
        }
    }

    /// How many characters are covered.
    pub fn len(&self) -> usize {
        match self {
            Self::Cached(set) => set.len(),
            Self::Owned(set) => set.len(),
        }
    }

    /// Whether nothing is covered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The covered characters, ascending.
    pub fn chars(self) -> CharsIter<'a> {
        match self {
            Self::Cached(set) => CharsIter::Cached(Box::new(set.chars())),
            Self::Owned(set) => CharsIter::Owned(Box::new(set.chars())),
        }
    }

    /// The covered characters as contiguous inclusive ranges, ascending.
    pub fn ranges(self) -> impl Iterator<Item = (char, char)> + 'a {
        let mut chars = self.chars();
        let mut pending = chars.next();
        std::iter::from_fn(move || {
            let start = pending?;
            let mut end = start;
            loop {
                match chars.next() {
                    Some(next) if next as u32 == end as u32 + 1 => end = next,
                    other => {
                        pending = other;
                        return Some((start, end));
                    }
                }
            }
        })
    }

    /// Check the structure, for coverage that has one to check.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Cached(set) => set.validate(),
            Self::Owned(_) => Ok(()),
        }
    }
}

/// Iterator over the characters of a [`Chars`].
pub enum CharsIter<'a> {
    /// Walking a cache's bitmap.
    Cached(Box<dyn Iterator<Item = char> + 'a>),
    /// Walking an in-memory set.
    Owned(Box<dyn Iterator<Item = char> + 'a>),
}

impl Iterator for CharsIter<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::Cached(iter) | Self::Owned(iter) => iter.next(),
        }
    }
}

impl PartialEq for Chars<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.chars().eq(other.chars())
    }
}

impl Eq for Chars<'_> {}

/// The form `fc-query` prints coverage in: inclusive hex ranges.
impl std::fmt::Display for Chars<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (start, end)) in self.ranges().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            if start == end {
                write!(f, "{:x}", start as u32)?;
            } else {
                write!(f, "{:x}-{:x}", start as u32, end as u32)?;
            }
        }
        Ok(())
    }
}
