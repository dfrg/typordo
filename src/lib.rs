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
mod matching;
mod md5;
mod object;
mod pattern;
mod query;
mod value;
mod xml;

pub use cache::{Cache, Fonts, Subdirs, MAGIC_ALLOC, MAGIC_MMAP, VERSION};
pub use config::{Caches, Config, ConfigError, ARCHITECTURE};
pub use error::{Error, Result};
pub use matching::{best, score, sorted, Priority, Score, PRIORITIES};
pub use object::Object;
pub use pattern::{Bindings, Element, Elements, Pattern, Values};
pub use query::{OwnedValue, Query};
pub use charset::CharSet;
pub use langset::{LangResult, LangSet};
pub use value::{Binding, Matrix, Range, Value};
