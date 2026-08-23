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
//! There is no `unsafe` in this crate unless one of the two optional features
//! that need it is on, and then only for a single call each: mapping a file
//! for `mmap`, and `statfs` for the feature of that name. The file is never
//! transmuted or
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
//! portable serialization. A file is only meaningful to a build laid out the
//! same way, which is why fontconfig puts the shape in the name:
//! `<hash>-le64.cache-9`.
//!
//! This crate derives its layout from the target it is compiled for, so a
//! build for a 32-bit machine reads and writes what that machine's own
//! fontconfig does -- `le32d4` on i386, `le32d8` on 32-bit ARM, where the
//! difference is whether a `double` is aligned to one word or two. See
//! [`ARCHITECTURE`] and the `layout` module.
//!
//! Byte order is the one axis not translated. A cache written on a
//! big-endian machine is not something this crate rejects, it is something
//! it never looks for: the name it asks for carries its own endianness, so
//! the two never meet. Reading a foreign-endian cache would mean byte
//! swapping every field for no benefit, since a cache is written by the
//! machine that uses it.
//!
//! Only format version 9 is read, which is what fontconfig 2.17 writes.
//!
//! # What has been verified where
//!
//! Every claim about fontconfig in this crate is checked against fontconfig
//! itself -- `fc-list`, `fc-match`, `fc-query`, `fc-cache` -- on one machine:
//! Fedora 44, x86_64, 695 font files. That is the only place an oracle
//! exists. Windows runs the test suite and the parity harnesses that do not
//! need fontconfig.
//!
//! The 32-bit layouts are derived rather than measured. They are checked
//! against the five closed forms `fcarch.c` states, for every pointer and
//! alignment pair, and the crate compiles for `i686` and `armv7` with those
//! assertions live -- but no cache written by a 32-bit fontconfig has ever
//! been read by this code. Treat those targets as untested rather than
//! unsupported.

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
pub use build::{Builder, Built};
#[cfg(feature = "scan")]
pub use scan::{scan_bytes, scan_file, ScanError};
pub use rules::{
    BinaryOp, Compare, Edit, EditMode, Expr, MatchKind, Qual, Rule, Step, Test, UnaryOp,
};
pub use langset::{LangResult, LangSet, Langs, Languages};
pub use value::{Binding, Matrix, Range, Value};
pub use write::CacheWriter;
