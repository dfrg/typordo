# fontconf

Fontconfig, without libfontconfig.

A from-scratch Rust implementation of what fontconfig does: scanning font
files, reading and writing its cache format, parsing its configuration, and
matching fonts. Closely enough that fontconfig reads the caches this writes,
and picks the same font.

```rust
use fontconf::{best, Config, Object, Pattern, Query};

let config = Config::load()?;

// Caches own the bytes; patterns borrow from them.
let caches: Vec<_> = config.caches().collect();
let fonts: Vec<Pattern<'_>> = caches
    .iter()
    .filter_map(|(_, cache)| cache.fonts().ok())
    .flatten()
    .filter(|font| config.accepts(font))
    .collect();

let mut query = Query::new();
query.add(Object::Family, "sans-serif");
query.add(Object::Lang, "ja");
config.substitute(&mut query);   // apply the config's <match> rules
query.default_substitute();      // fill in what the query left unsaid

if let Some((font, _score)) = best(&query, fonts.iter().cloned()) {
    println!("{:?}", font.string(Object::File));
}
```

## Examples

```
cargo run --example pick_font -- sans-serif ja こんにちは
cargo run --example fallback_chain -- "Hello Ελλάς 日本語 🎉 مرحبا"
```

`pick_font` answers the question fontconfig exists for: given a generic
family, a language and some characters, which installed font should render
it? `fallback_chain` builds the ordered list a layout engine walks, charging
each character to the first font that covers it.

The rest — `fc_list`, `fc_match`, `fc_query`, `fc_cache` — are CLI clones
built for parity testing. They take the same arguments as the originals and
exist to be diffed against them.

## Why it agrees with fontconfig

Every claim about fontconfig in this crate is checked against fontconfig
itself. Not against the documentation, which is thin, and not only against
the source, which does not tell you what a real font set does to it: against
`fc-list`, `fc-match`, `fc-query` and `fc-cache` running on the same machine
over the same fonts.

`scripts/` holds those harnesses, one per surface. They are the reason the
parity numbers below are numbers rather than adjectives.

The corpus is Fedora 44 under WSL: 2385 font files producing 2999 patterns
across 336 primary family names and 281 languages, with 378 configuration
files. Broad — CJK collections, Type 1, variable fonts, OpenType bitmaps,
colour emoji, and a Noto face for nearly every script there is — but one
machine, which is the honest limit on all of it.

## Status

Everything measured agrees, with one deliberate exception. What is knowingly
missing is written down in [docs/gaps.md](docs/gaps.md), so that "all parity
checks pass" can be read for what it is.

The exception: fontconfig drops the `capability` property for a named
instance of a font collection, because `ftglue_face_goto_table` seeks to
`12 + face->face_index * 4` without masking off the instance bits FreeType
packs into the high half. Three files. Reproducing it would mean withholding
correct data, so this does not.

## Performance

Measured against libfontconfig on the same corpus, same machine, with
checksums on both sides to show they did the same work. `scripts/bench.sh`
and `scripts/bench_fc.c` are the two drivers.

| operation | ours | fontconfig | |
| --- | --- | --- | --- |
| open a config | 6.80 ms | 13.70 ms | **2.01x** |
| load every cache | 10.77 ms | 15.86 ms | **1.47x** |
| list every font | 1.41 ms | 2.14 ms | **1.52x** |
| prepare a query | 1.08 ms | 425 us | 0.39x |
| match | 2.04 ms | 1.21 ms | 0.59x |
| sort | 3.82 ms | 2.89 ms | 0.76x |

Faster at everything that touches a cache, slower at everything that runs the
configuration or scores a font. The `prepare` column is the honest one to
look at: it is config rules alone, no fonts involved, and this machine has
378 of them. Fontconfig's rule engine scales better than this one does, and
that cost is inherited by `match` and `sort`, which run `prepare` first.

## Design

**Borrowing.** A cache owns its file's bytes and nothing else. Patterns,
elements and values are cursors into that buffer, and the strings they yield
are `&str` slices of it, so walking a cache allocates nothing after the
initial read.

**Safety.** No `unsafe` unless one of two optional features is on, and then
for a single call each. The buffer is never transmuted: every field is read
byte-wise through a bounds-checked accessor, so it needs no particular
alignment, and a corrupt file yields an `Error` rather than a crash. That
matters because a cache is shared mutable state — `/var/cache/fontconfig` is
world-readable, and any package installation can rewrite it under a reader.

**Format compatibility.** A cache is a memory image of fontconfig's
structures, not a portable serialization, so it is only meaningful to a build
laid out the same way. Fontconfig puts that shape in the name:
`<hash>-le64.cache-9`. This crate derives its layout from the target it
compiles for, so a 32-bit build reads and writes what that machine's own
fontconfig does. Byte order is the one axis not translated, because it never
has to be — the filename carries its own endianness, so a foreign-endian
cache is not rejected so much as never looked for.

Only version 9 is read, which is what fontconfig 2.17 writes.

## Features

| feature | default | dependency | what it buys |
| --- | --- | --- | --- |
| `scan` | yes | `read-fonts` | Building cache entries from font files, rather than only reading them. |
| `mmap` | no | `memmap2` | Map a cache instead of reading it in. Shares one copy between processes, and opens a large cache in O(1). |
| `statfs` | no | `libc` | Ask whether a filesystem's timestamps can be trusted, as fontconfig does for FAT. |
| `full-fontconfig-compat` | no | both | `mmap` and `statfs` together. |

Every dependency is optional. With no features the crate builds with none at
all and contains no `unsafe`; `mmap` and `statfs` each introduce exactly one
`unsafe` call, and `Cargo.toml` documents what each costs.

## Testing

```
cargo test                  # 230 tests, no fontconfig needed
scripts/run_tests_wsl.sh    # the same across the feature matrix, plus clippy
scripts/all_parity.sh       # every harness against live fontconfig
```

The parity harnesses need a Linux machine with fontconfig installed and
`--release`, since they run the whole corpus through both implementations.
Windows runs the test suite and the harnesses that do not need fontconfig.

The 32-bit layouts are derived rather than measured: checked against the five
closed forms `fcarch.c` states and compiled for `i686` and `armv7` with those
assertions live, but no cache written by a 32-bit fontconfig has ever been
read by this code. Treat those targets as untested, not unsupported.

## Generated tables

Five modules are generated from upstream data by `tools/gen_*.py` — the
language list and its orthographies, Unicode case folding, ZapfDingbats glyph
names, and the OpenType name-language table. Each generator reproduces its
output byte-identically; that is checked, because two of them had silently
drifted from hand-edits before it was.
