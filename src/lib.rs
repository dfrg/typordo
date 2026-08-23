//! Fontconfig, without libfontconfig.
//!
//! Scans font files, reads and writes fontconfig's own cache format, parses
//! its configuration, and matches fonts the way it does -- closely enough
//! that fontconfig reads the caches this writes, and picks the same font.
//!
//! ```no_run
//! use fontconf::{best, Config, Object, Pattern, Query};
//!
//! let config = Config::load()?;
//!
//! // The caches own the bytes; the patterns borrow from them.
//! let caches: Vec<_> = config.caches().collect();
//! let fonts: Vec<Pattern<'_>> = caches
//!     .iter()
//!     .filter_map(|(_, cache)| cache.fonts().ok())
//!     .flatten()
//!     .filter(|font| config.accepts(font))
//!     .collect();
//!
//! let mut query = Query::new();
//! query.add(Object::Family, "sans-serif");
//! query.add(Object::Lang, "ja");
//! config.substitute(&mut query);
//! query.default_substitute();
//!
//! if let Some((font, _score)) = best(&query, fonts.iter().cloned()) {
//!     println!("{:?}", font.string(Object::File));
//! }
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! # Borrowing
//!
//! A [`Cache`] owns its file's bytes and nothing else. Patterns, elements and
//! values are cursors into that buffer and the strings they yield are `&str`
//! slices of it, so walking a cache allocates nothing after the initial read.
//!
//! # Safety
//!
//! There is no `unsafe` unless one of two optional features is on, and then
//! for a single call each: mapping a file for `mmap`, and `statfs`. The
//! buffer is never transmuted -- every field is read byte-wise through a
//! bounds-checked accessor, so it needs no particular alignment and a corrupt
//! file yields an [`Error`] rather than a crash.
//!
//! That matters because a cache is shared mutable state. `/var/cache/fontconfig`
//! is world-readable, and any package installation can rewrite it under a reader.
//!
//! Structure is checked lazily: [`Cache::new`] validates the header, and the
//! iterators skip records that do not hold up, so one bad font does not hide a
//! directory. [`Cache::validate`] is the strict pass that walks everything and
//! reports the first problem instead.
//!
//! # Format compatibility
//!
//! A cache is a memory image of fontconfig's structures, not a portable
//! serialization, so it is only meaningful to a build laid out the same way.
//! Fontconfig puts that shape in the name: `<hash>-le64.cache-9`.
//!
//! This crate derives its layout from the target it compiles for, so a 32-bit
//! build reads and writes what that machine's own fontconfig does -- `le32d4`
//! on i386, `le32d8` on 32-bit ARM, differing in whether a `double` aligns to
//! one word or two. See [`ARCHITECTURE`].
//!
//! Byte order is the one axis not translated, because it never has to be: the
//! filename carries its own endianness, so a foreign-endian cache is not
//! rejected so much as never looked for. Swapping every field would buy
//! nothing, since a cache is written by the machine that uses it.
//!
//! Only version 9 is read, which is what fontconfig 2.17 writes.
//!
//! # What is verified, and where
//!
//! Every claim about fontconfig here is checked against fontconfig itself --
//! `fc-list`, `fc-match`, `fc-query`, `fc-cache` -- on one machine: Fedora 44,
//! x86_64, 2385 font files. That is the only place an oracle exists. Windows
//! runs the test suite and the harnesses that do not need fontconfig.
//!
//! The 32-bit layouts are derived rather than measured. They are checked
//! against the five closed forms `fcarch.c` states, for every pointer and
//! alignment pair, and the crate compiles for `i686` and `armv7` with those
//! assertions live -- but no cache written by a 32-bit fontconfig has ever
//! been read by this code. Treat those targets as untested, not unsupported.

// Two optional features need `unsafe`, each for exactly one call: `mmap` to
// map a cache file, and `statfs` to ask what kind of filesystem a directory
// is on. With neither of them the ban is absolute.
#![cfg_attr(not(any(feature = "mmap", feature = "statfs")), forbid(unsafe_code))]
#![cfg_attr(any(feature = "mmap", feature = "statfs"), deny(unsafe_code))]
#![warn(missing_docs)]

#[cfg(feature = "scan")]
mod build;
mod bytes;
mod cache;
pub mod casefold;
mod charset;
mod config;
mod error;
mod fnv;
mod glob;
pub mod langs;
mod langset;
mod layout;
mod matching;
mod md5;
#[cfg(feature = "scan")]
mod name_langs;
mod object;
mod orth;
mod pattern;
mod prepare;
mod query;
mod rules;
#[cfg(feature = "scan")]
mod scan;
mod value;
pub mod weight;
mod write;
mod xml;
mod zapf;

#[cfg(feature = "scan")]
pub use build::{Builder, Built};
pub use cache::{Cache, Fonts, Subdirs, VERSION};
pub use charset::{CharSet, Chars, Coverage};
pub use config::{Caches, Config, ConfigError, ARCHITECTURE};
pub use error::{Error, Result};
pub use langset::{LangResult, LangSet, Langs, Languages};
pub use matching::{best, best_value, score, sort, sorted, BestValue, Priority, Score, PRIORITIES};
pub use object::Object;
pub use pattern::{Bindings, Element, Elements, Pattern, Values};
pub use prepare::render_prepare;
pub use query::{default_langs, OwnedValue, Query};
pub use rules::MatchKind;
#[cfg(feature = "scan")]
pub use scan::{scan_bytes, scan_file, ScanError};
pub use value::{Binding, Matrix, Range, Value};
pub use write::CacheWriter;
