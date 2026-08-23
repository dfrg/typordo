//! A reader for fontconfig's font cache files.
//!
//! Fontconfig scans font directories and writes what it found to a cache, one
//! file per directory, under `/var/cache/fontconfig` and `~/.cache/fontconfig`.
//! Every application on the system reads those caches instead of re-parsing
//! the fonts. This crate reads them too, without linking to libfontconfig.
//!
//! ```no_run
//! use fontconf::{Cache, Object};
//!
//! let cache = Cache::open("/home/me/.cache/fontconfig/abc-le64.cache-9")?;
//! println!("{}", cache.dir()?);
//! for font in cache.fonts()? {
//!     if let (Some(family), Some(file)) =
//!         (font.string(Object::Family), font.string(Object::File))
//!     {
//!         println!("{family}: {file}");
//!     }
//! }
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! # Borrowing
//!
//! A [`Cache`] owns the file's bytes and nothing else. Patterns, elements and
//! values are cursors into that buffer, and the strings they yield are
//! `&str` slices of it, so walking a cache allocates nothing after the
//! initial read.
//!
//! # Safety and trust
//!
//! There is no `unsafe` in this crate. The file is never transmuted or
//! reinterpreted; every field is read byte-wise through a bounds-checked
//! accessor, so the buffer needs no particular alignment and a corrupt file
//! produces an [`Error`], never a crash. That matters because a cache is
//! shared mutable state: `/var/cache/fontconfig` is world-readable and any
//! package installation can rewrite it underneath a reader.
//!
//! Structure is checked lazily. [`Cache::new`] validates the header, and the
//! iterators skip records that do not hold up, so one bad font does not hide
//! a directory. [`Cache::validate`] is the strict pass that walks everything
//! and reports the first problem instead.
//!
//! # Format compatibility
//!
//! Cache files are memory images of fontconfig's internal structures, not a
//! portable serialization. A file is only meaningful to a build with the same
//! format version, word size and byte order, which is why fontconfig puts all
//! three in the name: `<hash>-le64.cache-9`. This crate reads 64-bit
//! little-endian version 9, the format fontconfig 2.17 writes, and refuses
//! anything else rather than misreading it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bytes;
mod cache;
pub mod casefold;
mod charset;
mod config;
mod error;
mod glob;
pub mod langs;
mod langset;
pub mod orth;
mod matching;
mod md5;
#[cfg(feature = "scan")]
mod name_langs;
mod object;
mod pattern;
mod prepare;
mod query;
mod rules;
#[cfg(feature = "scan")]
mod scan;
mod value;
mod write;
pub mod weight;
mod xml;
mod zapf;

pub use cache::{Cache, Fonts, Subdirs, MAGIC_ALLOC, MAGIC_MMAP, VERSION};
pub use config::{Caches, Config, ConfigError, ARCHITECTURE};
pub use error::{Error, Result};
pub use charset::{CharSet, Chars, Coverage};
pub use matching::{
    best, best_value, score, sort, sorted, BestValue, Priority, Score, PRIORITIES,
};
pub use object::Object;
pub use pattern::{Bindings, Element, Elements, Pattern, Values};
pub use prepare::render_prepare;
pub use query::{default_langs, OwnedValue, Property, Query};
#[cfg(feature = "scan")]
pub use scan::{scan_bytes, scan_file, ScanError};
pub use rules::{
    BinaryOp, Compare, Edit, EditMode, Expr, MatchKind, Qual, Rule, Step, Test, UnaryOp,
};
pub use langset::{LangResult, LangSet, Langs, Languages};
pub use value::{Binding, Matrix, Range, Value};
pub use write::CacheWriter;
