//! The set of characters a font can draw.
//!
//! Fontconfig stores coverage as a sparse two-level bitmap: an ascending list
//! of 256-codepoint pages, and one 256-bit leaf per page. A font covering
//! Latin-1 and nothing else carries two leaves, not a bitmap over all of
//! Unicode.

use crate::bytes::Bytes;
use crate::error::{Error, Result};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

use crate::layout::{self, NATIVE as L};

/// The characters one leaf word stands for, from the lowest.
///
/// Walks the bits that are *set* rather than testing all thirty-two. A leaf
/// holds 256 codepoints and a query naming a handful of them was testing
/// every one of those bits to find them, for every font in the set.
fn set_bits(mut bits: u32, base: u32) -> impl Iterator<Item = char> {
    std::iter::from_fn(move || loop {
        if bits == 0 {
            return None;
        }
        let bit = bits.trailing_zeros();
        // Clear the lowest set bit and move on.
        bits &= bits - 1;
        // A surrogate is not a character. Nothing should have one, but a
        // corrupt cache can, and it must not end the walk early.
        if let Some(c) = char::from_u32(base + bit) {
            return Some(c);
        }
    })
}

/// A leaf covers 256 codepoints as eight 32-bit words.
pub(crate) const LEAF_WORDS: usize = 8;
const LEAF_BYTES: usize = LEAF_WORDS * 4;
/// Codepoints per page.
pub(crate) const PAGE: u32 = 256;

/// The characters a font covers, read from a cache.
///
/// One of three types for the same idea, told apart by where the bits live:
/// this borrows them from a cache, [`OwnedCharSet`] holds its own and can
/// grow, and [`CharSetRef`] is either of those seen through a reference.
/// Matching and reporting take the last, so they do not care which.
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
            let leaf = set.leaf(index).unwrap_or([0; LEAF_WORDS]);
            (0..LEAF_WORDS).flat_map(move |word| set_bits(leaf[word], base + word as u32 * 32))
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
        self.data.array(leaves, pages, layout::PTR)?;
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
        self.data.count(self.at + L.charset_num)
    }

    fn numbers_base(&self) -> Result<usize> {
        self.data.resolve(self.at, self.data.offset(self.at + L.numbers)?)
    }

    fn leaves_base(&self) -> Result<usize> {
        self.data.resolve(self.at, self.data.offset(self.at + L.leaves)?)
    }

    /// The page number stored at `index`.
    pub(crate) fn page_number(&self, index: usize) -> Result<u16> {
        let at = self.numbers_base()? + index * 2;
        // Two bytes, read as two: masking a four-byte read would take the
        // wrong half of it on a big-endian machine.
        self.data.u16(at)
    }

    /// How many of `chars` this set does not cover.
    ///
    /// This is the whole of `FcCharSetSubtractCount` for the shape that
    /// matters: a fallback picker names a handful of characters it needs and
    /// asks every font in the set whether it has them. Scoring therefore
    /// calls this once per font, and [`CharSet::contains`] would re-resolve
    /// the two page arrays for every character of every one of them.
    /// Resolved once here instead.
    pub(crate) fn missing_count(&self, chars: impl Iterator<Item = char>) -> usize {
        let (Ok(pages), Ok(numbers), Ok(leaves)) =
            (self.checked_pages(), self.numbers_base(), self.leaves_base())
        else {
            // A set that cannot be read covers nothing, so everything asked
            // for is missing.
            return chars.count();
        };

        // Characters arrive in ascending order -- a charset yields them that
        // way -- and the handful a fallback query carries are sampled from
        // one script, so they share a page or two. Remembering the last page
        // located turns a binary search per character into one search and a
        // comparison for the rest, which is worth most against a CJK font
        // whose page array is thousands long.
        //
        // Order is an optimisation, not an assumption: a character that goes
        // backwards reopens the window rather than being missed.
        let mut missing = 0usize;
        let mut low = 0usize;
        let mut last: Option<(u16, Option<usize>)> = None;

        for c in chars {
            let Ok(page) = u16::try_from(c as u32 / PAGE) else {
                missing += 1;
                continue;
            };
            let index = match last {
                Some((seen, index)) if seen == page => index,
                other => {
                    if other.is_some_and(|(seen, _)| page < seen) {
                        low = 0;
                    }
                    let (index, next) = match self.page_index(pages, numbers, page, low) {
                        Ok(at) => (Some(at), at),
                        Err(at) => (None, at),
                    };
                    low = next;
                    last = Some((page, index));
                    index
                }
            };
            match index {
                Some(index) if self.bit_set(leaves, index, c) => {}
                _ => missing += 1,
            }
        }
        missing
    }

    /// Where `page` sits in the page array, or where it would be inserted.
    ///
    /// `low` bounds the search from below, which is what lets a run of
    /// ascending characters keep narrowing it.
    fn page_index(
        &self,
        pages: usize,
        numbers: usize,
        page: u16,
        low: usize,
    ) -> std::result::Result<usize, usize> {
        let (mut low, mut high) = (low.min(pages), pages);
        while low < high {
            let mid = low + (high - low) / 2;
            let Ok(at) = self.data.u16(numbers + mid * 2) else { return Err(low) };
            match at.cmp(&page) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Ok(mid),
            }
        }
        Err(low)
    }

    /// Whether the leaf at `index` has the bit for `c`.
    fn bit_set(&self, leaves: usize, index: usize, c: char) -> bool {
        let Ok(delta) = self.data.offset(leaves + index * layout::PTR) else { return false };
        let Ok(leaf) = self.data.resolve(leaves, delta) else { return false };
        let word = ((c as u32 % PAGE) / 32) as usize;
        let Ok(bits) = self.data.u32(leaf + word * 4) else { return false };
        bits & (1 << (c as u32 % 32)) != 0
    }

    /// A whole leaf.
    pub(crate) fn leaf(&self, index: usize) -> Result<[u32; LEAF_WORDS]> {
        self.leaf_at(self.leaves_base()?, index)
    }

    /// A whole leaf, given an already-resolved leaf array base.
    ///
    /// The base is a parameter because the caller that matters resolves it
    /// once for a whole font: [`CharSet::leaf_word`] re-resolves it for every
    /// word it reads, which is eight times per page, for every page of every
    /// candidate a fallback list considers.
    pub(crate) fn leaf_at(&self, leaves: usize, index: usize) -> Result<[u32; LEAF_WORDS]> {
        let delta = self.data.offset(leaves + index * layout::PTR)?;
        let leaf = self.data.resolve(leaves, delta)?;
        let mut out = [0u32; LEAF_WORDS];
        for (word, slot) in out.iter_mut().enumerate() {
            *slot = self.data.u32(leaf + word * 4)?;
        }
        Ok(out)
    }

    /// One 32-bit word of leaf `index`.
    ///
    /// Leaf offsets are relative to the start of the leaf array, the same
    /// convention the cache's subdirectory list uses.
    pub(crate) fn leaf_word(&self, index: usize, word: usize) -> Result<u32> {
        let leaves = self.leaves_base()?;
        let delta = self.data.offset(leaves + index * layout::PTR)?;
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

/// A set of characters held in memory, which can grow.
///
/// The owned counterpart to [`CharSet`]: that one borrows a cache's bits,
/// this one holds its own. [`CharSetRef`] is either.
///
/// Sorting a font list needs to know whether each font adds anything the ones
/// before it did not, which means accumulating coverage as the walk proceeds.
/// The layout mirrors [`CharSet`]'s -- 256-codepoint pages of eight words --
/// so merging a font is a handful of word-ORs per page rather than a pass over
/// its characters.
#[derive(Default)]
pub struct OwnedCharSet {
    /// Pages in ascending order, which is how a cache stores them and how
    /// every reader of this wants them.
    ///
    /// A hash map was the obvious choice and the wrong one. Sorting is what
    /// the serialized form needs, what `chars` needs, and what lets `merge`
    /// walk two sets in step instead of hashing every page of one of them --
    /// and hashing a `u16` with SipHash was a sixth of the cost of building a
    /// fallback list. Fontconfig keeps a sorted array for the same reasons.
    pages: Vec<(u16, Leaf)>,
    /// Where the last insert landed, since characters arrive in runs.
    ///
    /// Atomic only so that a set can be shared between threads, which a C
    /// caller through an FFI would expect of anything reachable from a
    /// pattern. Relaxed throughout: this is a hint, and a stale or torn one
    /// costs a binary search that would otherwise have been skipped, never a
    /// wrong answer.
    ///
    /// A font's cmap walks upwards, so the next character is almost always on
    /// the page the last one was, and that turns a binary search into a
    /// comparison.
    recent: AtomicUsize,
    /// A spare page list, swapped with `pages` when a merge rebuilds it.
    ///
    /// Building a fallback list merges hundreds of fonts into one set, so the
    /// two buffers ping-pong and keep their capacity instead of allocating a
    /// destination per merge.
    scratch: Vec<(u16, Leaf)>,
}

/// One page of coverage: 256 codepoints as eight words.
type Leaf = [u32; LEAF_WORDS];

/// Only the coverage counts. `recent` is a memo of where the last lookup
/// landed, so two sets covering the same characters are the same set whatever
/// each was last asked about.
impl PartialEq for OwnedCharSet {
    fn eq(&self, other: &Self) -> bool {
        self.pages == other.pages
    }
}

impl Eq for OwnedCharSet {}

/// The scratch buffer is working space, not content: a clone starts without
/// one rather than carrying a copy of whatever the last merge left behind.
impl Clone for OwnedCharSet {
    fn clone(&self) -> Self {
        Self {
            pages: self.pages.clone(),
            recent: AtomicUsize::new(self.recent.load(Relaxed)),
            scratch: Vec::new(),
        }
    }
}

impl std::fmt::Debug for OwnedCharSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OwnedCharSet({} chars in {} pages)", self.len(), self.pages.len())
    }
}

impl OwnedCharSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where `page` is, or where it would be inserted.
    fn find(&self, page: u16) -> std::result::Result<usize, usize> {
        // The run cache first: a scan inserts thousands of characters in
        // ascending order, and they share a page 255 times out of 256.
        let recent = self.recent.load(Relaxed);
        if let Some((at, _)) = self.pages.get(recent) {
            if *at == page {
                return Ok(recent);
            }
        }
        let found = self.pages.binary_search_by_key(&page, |(at, _)| *at);
        if let Ok(index) = found {
            self.recent.store(index, Relaxed);
        }
        found
    }

    /// The leaf for `page`, creating an empty one if there is none.
    fn leaf_mut(&mut self, page: u16) -> &mut Leaf {
        let index = match self.find(page) {
            Ok(index) => index,
            Err(index) => {
                self.pages.insert(index, (page, [0; LEAF_WORDS]));
                self.recent.store(index, Relaxed);
                index
            }
        };
        &mut self.pages[index].1
    }

    /// Add everything in a set of characters, however it is stored.
    ///
    /// The bool is the whole point: it is what decides that a font earns its
    /// place in a fallback list.
    pub fn merge_chars(&mut self, other: &CharSetRef<'_>) -> bool {
        match other {
            CharSetRef::Cached(set) => self.merge(set),
            CharSetRef::Owned(set) => {
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
        // Building a fallback list merges every candidate font into a set
        // that only grows, so this runs hundreds of times against something
        // thousands of pages long. Pages already here are updated where they
        // sit; only a set that brings pages we have never seen rebuilds the
        // list, and trimming means that stops happening early.
        let mut added = false;
        let mut unseen = 0usize;
        let mut cursor = 0usize;

        // The two array bases are resolved once. Reading them per page --
        // which `page_number` and `leaf` each do -- is two offset decodes and
        // two bounds checks for every one of the thousands of pages a CJK
        // font has, repeated for every font a fallback list considers.
        let (Ok(numbers), Ok(leaves)) = (other.numbers_base(), other.leaves_base()) else {
            return false;
        };

        for index in 0..other.pages() {
            let Ok(page) = other.data.u16(numbers + index * 2) else { continue };
            let Ok(theirs) = other.leaf_at(leaves, index) else { continue };

            // Both sides ascend, so the cursor only ever moves forwards.
            while self.pages.get(cursor).is_some_and(|(at, _)| *at < page) {
                cursor += 1;
            }
            match self.pages.get_mut(cursor) {
                Some((at, leaf)) if *at == page => {
                    for (slot, bits) in leaf.iter_mut().zip(theirs.iter()) {
                        // Anything set there that was not set here is new.
                        if bits & !*slot != 0 {
                            added = true;
                        }
                        *slot |= bits;
                    }
                }
                _ => {
                    if theirs.iter().any(|word| *word != 0) {
                        added = true;
                    }
                    unseen += 1;
                }
            }
        }

        if unseen == 0 {
            return added;
        }

        // Both lists ascend, so the new pages are woven in with a single pass
        // rather than appended and re-sorted. Sorting was O(n log n) over
        // thousands of pages to place a handful, and it moved the whole list
        // twice to do it.
        let mut merged = std::mem::take(&mut self.scratch);
        merged.clear();
        merged.reserve(self.pages.len() + unseen);

        let mut mine = 0usize;
        for index in 0..other.pages() {
            let Ok(page) = other.data.u16(numbers + index * 2) else { continue };
            let Ok(theirs) = other.leaf_at(leaves, index) else { continue };

            // Everything of ours that sorts before this page passes through,
            // including the page itself if we have it -- phase one already
            // merged the bits into it.
            while let Some(&(at, leaf)) = self.pages.get(mine) {
                if at > page {
                    break;
                }
                merged.push((at, leaf));
                mine += 1;
                if at == page {
                    break;
                }
            }
            if merged.last().map(|(at, _)| *at) != Some(page) {
                merged.push((page, theirs));
            }
        }
        merged.extend_from_slice(&self.pages[mine..]);

        std::mem::swap(&mut self.pages, &mut merged);
        self.scratch = merged;
        self.recent.store(0, Relaxed);
        added
    }

    /// Whether `c` is covered.
    pub fn contains(&self, c: char) -> bool {
        let page = (c as u32 / PAGE) as u16;
        let Ok(index) = self.find(page) else { return false };
        let leaf = &self.pages[index].1;
        leaf[((c as u32 % PAGE) / 32) as usize] & (1 << (c as u32 % 32)) != 0
    }

    /// How many characters the union holds.
    pub fn len(&self) -> usize {
        self.pages
            .iter()
            .map(|(_, leaf)| leaf.iter().map(|w| w.count_ones() as usize).sum::<usize>())
            .sum()
    }

    /// Whether nothing has been merged in.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Everything in either set.
    pub fn union(&self, other: &Self) -> Self {
        let mut out = self.clone();
        for (page, leaf) in &other.pages {
            let slot = out.leaf_mut(*page);
            for (word, bits) in slot.iter_mut().zip(leaf.iter()) {
                *word |= bits;
            }
        }
        out
    }

    /// Everything in this set that is not in `other`.
    ///
    /// A page that empties out is dropped rather than kept as zeroes: the
    /// serialized form counts pages, so an empty leaf would be a difference
    /// that no character accounts for.
    pub fn subtract(&self, other: &Self) -> Self {
        let mut out = Self::new();
        for (page, leaf) in &self.pages {
            let mut result = *leaf;
            if let Ok(index) = other.find(*page) {
                for (word, bits) in result.iter_mut().zip(other.pages[index].1.iter()) {
                    *word &= !bits;
                }
            }
            if result.iter().any(|word| *word != 0) {
                // Both sides ascend, so this only ever appends.
                out.pages.push((*page, result));
            }
        }
        out
    }

    /// The pages of coverage in the order a cache stores them, ascending.
    pub(crate) fn leaves(&self) -> &[(u16, Leaf)] {
        &self.pages
    }

    /// Add one character.
    pub fn insert(&mut self, c: char) {
        let page = (c as u32 / PAGE) as u16;
        let leaf = self.leaf_mut(page);
        leaf[((c as u32 % PAGE) / 32) as usize] |= 1 << (c as u32 % 32);
    }

    /// The covered characters, ascending.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.pages.iter().flat_map(move |(page, leaf)| {
            let base = u32::from(*page) * PAGE;
            leaf.iter()
                .enumerate()
                .flat_map(move |(word, bits)| set_bits(*bits, base + word as u32 * 32))
        })
    }

    /// Whether every character in `ranges` is covered.
    ///
    /// This is the question a language orthography asks.
    pub fn covers_ranges(&self, ranges: &[(u32, u32)]) -> bool {
        ranges
            .iter()
            .all(|(lo, hi)| (*lo..=*hi).all(|c| char::from_u32(c).is_none_or(|c| self.contains(c))))
    }
}

/// A reference to a set of characters, whichever way it is stored.
///
/// Coverage read from a cache borrows its bytes; coverage produced by
/// scanning a font is built in memory. Both answer the same questions, so
/// matching and reporting take this rather than one or the other.
///
/// Both arms are borrows -- a cache cursor, or a reference to an
/// [`OwnedCharSet`] -- which is what keeps this `Copy`. Scoring clones it
/// per font, so it must stay cheap.
#[derive(Clone, Copy, Debug)]
pub enum CharSetRef<'a> {
    /// Read from a cache.
    Cached(CharSet<'a>),
    /// Built by scanning a font.
    Owned(&'a OwnedCharSet),
}

impl<'a> CharSetRef<'a> {
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

/// Iterator over the characters of a [`CharSetRef`].
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

impl PartialEq for CharSetRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.chars().eq(other.chars())
    }
}

impl Eq for CharSetRef<'_> {}

/// The form `fc-query` prints coverage in: inclusive hex ranges.
impl std::fmt::Display for CharSetRef<'_> {
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

/// Set arithmetic on the owned form.
#[cfg(test)]
mod set_tests {
    use super::OwnedCharSet;

    fn coverage(chars: &str) -> OwnedCharSet {
        let mut set = OwnedCharSet::new();
        for c in chars.chars() {
            set.insert(c);
        }
        set
    }

    #[test]
    fn union_takes_everything_from_both() {
        let joined = coverage("abc").union(&coverage("cde"));
        assert_eq!(joined.chars().collect::<String>(), "abcde");
    }

    #[test]
    fn subtract_takes_only_what_the_other_has() {
        let left = coverage("abcde").subtract(&coverage("bd"));
        assert_eq!(left.chars().collect::<String>(), "ace");
    }

    /// A page that empties out is dropped, not kept as zeroes: the
    /// serialized form counts pages, so an all-zero leaf would be a
    /// difference no character accounts for.
    #[test]
    fn an_emptied_page_is_dropped() {
        let latin = coverage("abc");
        let han = coverage("\u{4e00}");
        let both = latin.union(&han);
        assert_eq!(both.subtract(&han), latin);
        assert_eq!(both.subtract(&han).len(), 3);
    }

    #[test]
    fn union_spans_pages() {
        let joined = coverage("a").union(&coverage("\u{4e00}\u{10000}"));
        assert_eq!(joined.len(), 3);
        assert!(joined.contains('\u{10000}'));
    }

    #[test]
    fn neither_operation_changes_its_operands() {
        let a = coverage("abc");
        let b = coverage("bcd");
        a.union(&b);
        a.subtract(&b);
        assert_eq!(a.chars().collect::<String>(), "abc");
        assert_eq!(b.chars().collect::<String>(), "bcd");
    }
}

/// Tests for the sorted page list, which replaced a hash map and has to
/// behave identically however the pages arrive.
#[cfg(test)]
mod page_tests {
    use super::OwnedCharSet;

    fn coverage(chars: &[char]) -> OwnedCharSet {
        let mut set = OwnedCharSet::new();
        for c in chars {
            set.insert(*c);
        }
        set
    }

    /// The pages have to come out ascending whatever order they went in,
    /// because the serialized form and the binary search both depend on it.
    #[test]
    fn pages_stay_sorted_however_they_arrive() {
        let ascending = coverage(&['a', '\u{4e00}', '\u{10000}']);
        let descending = coverage(&['\u{10000}', '\u{4e00}', 'a']);
        let jumbled = coverage(&['\u{4e00}', 'a', '\u{10000}']);

        let pages: Vec<u16> = ascending.leaves().iter().map(|(p, _)| *p).collect();
        assert_eq!(pages, [0, 78, 256]);
        assert_eq!(ascending, descending);
        assert_eq!(ascending, jumbled);
    }

    /// The run cache remembers where the last lookup landed. A lookup that
    /// jumps away from it must not be answered from it.
    #[test]
    fn the_run_cache_does_not_answer_the_wrong_page() {
        let set = coverage(&['a', '\u{4e00}']);
        for _ in 0..3 {
            assert!(set.contains('a'));
            assert!(set.contains('\u{4e00}'));
            assert!(!set.contains('\u{10000}'));
            assert!(!set.contains('\u{500}'));
        }
    }

    /// Inserting into a page that already exists must not add another.
    #[test]
    fn a_second_character_on_a_page_reuses_it() {
        let set = coverage(&['a', 'b', 'c']);
        assert_eq!(set.leaves().len(), 1);
        assert_eq!(set.len(), 3);
    }

    /// Two sets covering the same characters are equal whatever each was
    /// last asked about: the run cache is a memo, not part of the value.
    #[test]
    fn the_run_cache_is_not_part_of_equality() {
        let a = coverage(&['a', '\u{4e00}']);
        let b = coverage(&['a', '\u{4e00}']);
        a.contains('\u{4e00}');
        b.contains('a');
        assert_eq!(a, b);
    }

    /// Merging is in place for pages that exist and splices the rest, so
    /// both halves need checking -- including that `added` stays right.
    #[test]
    fn merging_reports_what_it_added() {
        let mut base = OwnedCharSet::new();
        let latin = coverage(&['a', 'b']);
        let han = coverage(&['\u{4e00}']);

        assert!(base.merge_chars(&crate::CharSetRef::Owned(&latin)), "an empty set gains");
        assert!(!base.merge_chars(&crate::CharSetRef::Owned(&latin)), "the same set adds nothing");
        assert!(base.merge_chars(&crate::CharSetRef::Owned(&han)), "a new page is new");
        assert_eq!(base.leaves().len(), 2);
        assert!(base.contains('a') && base.contains('\u{4e00}'));
    }
}

/// The page cursor in [`CharSet::missing_count`], against the search it
/// replaced.
#[cfg(test)]
mod cursor_tests {
    use crate::{Cache, Object, Value};

    fn cantarell() -> Cache {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/cantarell-le64.cache-9");
        Cache::open(&path).expect("fixture cache")
    }

    /// `missing_count` walks with a cursor, which only pays because a charset
    /// yields characters in ascending order. Order is an optimisation, not a
    /// precondition, so the answer has to be the same however they arrive.
    ///
    /// `contains` is the reference: it searches from scratch every time.
    #[test]
    fn missing_count_agrees_with_contains_in_any_order() {
        let cache = cantarell();
        let font = cache.fonts().unwrap().next().expect("a font");
        let Some(Value::CharSet(set)) = font.value(Object::Charset) else {
            panic!("expected a charset");
        };
        let crate::charset::CharSetRef::Cached(set) = set else {
            panic!("expected a cached charset");
        };

        // Covered and uncovered, adjacent pages and distant ones.
        let mut probes: Vec<char> =
            "Az ~\u{7f}\u{a0}\u{131}\u{4e00}\u{2020}\u{ffff}".chars().collect();
        probes.push('\u{10fffd}');

        let mut ascending = probes.clone();
        ascending.sort_unstable();
        let mut descending = ascending.clone();
        descending.reverse();
        // Each character twice, so a repeated page is exercised as well.
        let mut doubled = ascending.clone();
        doubled.extend(ascending.iter().copied());

        for (name, order) in [
            ("ascending", &ascending),
            ("descending", &descending),
            ("as written", &probes),
            ("doubled", &doubled),
        ] {
            let want = order.iter().filter(|c| !set.contains(**c)).count();
            let got = set.missing_count(order.iter().copied());
            assert_eq!(got, want, "{name}: {order:?}");
        }
    }
}
