# The fontconfig API, in this crate's terms

If you know libfontconfig, this says where each thing went. It is a reading
aid and a checklist, not a promise: nothing here commits to a C binding, and
the crate is not a drop-in replacement for `libfontconfig.so`.

It is kept because it is the cheapest way to notice drift. Every entry that
reads "no equivalent" is a decision — sometimes a deliberate one, sometimes a
gap nobody has filled — and having them in one place is how they stay
visible instead of being discovered by whoever needs them.

Fontconfig 2.17 exports 238 `Fc*` functions -- the names below were checked
against its public header, so anything cited here is really in the API. What follows covers the ones an
application actually calls, plus the ones whose absence is worth knowing
about.

## Two differences that explain most of the rest

**There is no current configuration.** Fontconfig has a process-global
`FcConfig` that most calls reach by passing `NULL`, kept up to date behind
your back by `FcInitBringUptoDate` on a 30-second timer. Here a
[`Config`](../src/config.rs) is a value you hold. Nothing refreshes it; drop
it and load another. That removes `FcInitReinitialize`, the rescan interval,
and the thread-safety questions that go with shared mutable global state.

**`FcPattern` is two types.** Fontconfig uses one mutable, reference-counted
type for a query being built, a font read from a cache, and the merged result
of matching. This crate splits it by where the data lives:

| | fontconfig | here |
| --- | --- | --- |
| a query you build | `FcPattern *` | `Query` — owned, mutable |
| a font from a cache | `FcPattern *` (`FC_REF_CONSTANT`) | `Pattern<'a>` — a 32-byte cursor, read-only |
| the result of matching | `FcPattern *` (new) | `Query` |

The split is why scoring allocates nothing: a `Pattern` is a cursor into the
cache's bytes and the strings it yields borrow from them. It is also the
thing any C binding would have to reconcile, since a `Pattern` cannot outlive
the `Cache` it reads. Fontconfig has the same constraint on `FcFontSetSort`'s
result and documents it; it just cannot express it in the type.

`Query::from_pattern` copies one into the other when a borrowed font needs to
outlive its cache.

## Starting up

| fontconfig | here |
| --- | --- |
| `FcInit` | — no global state to initialise |
| `FcInitLoadConfig` | `Config::load()` |
| `FcInitLoadConfigAndFonts` | `Config::load()` then `Config::build_fonts()` |
| `FcConfigCreate` / `Destroy` / `Reference` | ordinary ownership |
| `FcConfigGetCurrent` / `SetCurrent` | — no current configuration |
| `FcInitBringUptoDate` / `FcInitReinitialize` | — drop the `Config` and load again |
| `FcConfigGetRescanInterval` / `SetRescanInterval` | — nothing to refresh |
| `FcConfigBuildFonts` | `Config::build_fonts()` |
| `FcConfigGetFontDirs` | `Config::font_dirs()` |
| `FcConfigGetCacheDirs` | `Config::cache_dirs()` |
| `FcConfigGetConfigFiles` | `Config::files()` |
| `FcConfigUptoDate` | partly: `CachePolicy` decides, `Caches::skipped()` reports |

`FcConfigBuildFonts` scans and writes a cache for any directory that lacks a
current one, silently, which is why the first application to start after a
font is installed is slow. `build_fonts` does exactly that, but you have to
ask; `Config::caches(CachePolicy::read_only())` never scans or writes.

## Building and reading a pattern

| fontconfig | here |
| --- | --- |
| `FcPatternCreate` | `Query::new()` |
| `FcPatternDestroy` | drop |
| `FcPatternDuplicate` | `Query::clone()` |
| `FcPatternAddInteger` / `Double` / `String` / `Bool` / `Matrix` / `Range` / `CharSet` / `LangSet` | `Query::add(Object, value)` |
| `FcPatternAddWeak` | `Query::add_weak` |
| `FcPatternAdd` with a binding | `Query::add_with_binding` |
| `FcPatternDel` | `Query::remove` |
| `FcPatternGet` / `GetString` / `GetInteger` / … | `Query::value`, `string`, `number`, `get` |
| the same, on a cached font | `Pattern::value`, `string`, `int`, `get` |
| `FcPatternEqual` | `PartialEq` |
| `FcPatternObjectCount` | `Query::len`, `Pattern::len` |
| `FcPatternIterStart` / `IterNext` | `Query::elements`, `Pattern::elements` |
| `FcDefaultSubstitute` | `Query::default_substitute()` |
| `FcConfigSubstitute` | `Config::substitute()` |
| `FcConfigSubstituteWithPat` | `Config::substitute_kind(query, kind, Some(pattern))` |
| `FcNameParse` | **no equivalent** — see Gaps |
| `FcNameUnparse` / `FcPatternFormat` | **no equivalent** — see Gaps |
| `FcPatternFilter` / `EqualSubset` | — no equivalent |

Both rewrites have to run, in this order, before matching: `substitute`
resolves the configuration's aliases, `default_substitute` fills in what the
query left unsaid. Fontconfig requires the same sequence and scoring assumes
it in both.

## Matching

| fontconfig | here |
| --- | --- |
| `FcFontMatch` | `best()`, then `render_prepare()` |
| `FcFontSetMatch` | `best()` |
| `FcFontSort` | `sort(query, fonts, trim)` |
| `FcFontSetSort` | `sort()`, or `sorted()` for no trimming |
| `FcFontRenderPrepare` | `render_prepare()` |
| `FcFontSetCreate` / `Add` / `Destroy` | `Vec<Pattern>` |
| `FcFontList` / `FcFontSetList` | walk `Config::caches()` and filter |

`FcFontMatch` is `best` followed by `render_prepare`: the first picks the
font, the second merges it with the query into what a caller actually wants
— the family it matched under, the size asked for, the localized name that
fit. Skipping the second step is a common way to get a surprising answer.

Scoring order, tie-breaking and the language-satisfaction pass are the same,
checked against `fc-match` over the whole corpus rather than asserted; see
the parity table in the README.

Two names worth mentioning are fontconfig's internals rather than its API,
since anyone reading its source meets them: `FcCompare` in `fcmatch.c`, which
scores one font against one pattern, is `score()` here, or `best_value()` for
a single property; and `FcLangSetFromCharSet` in `fclang.c`, which works out
what a font's coverage implies about the languages it supports, is
`OwnedLangSet::from_char_set`.

## Character sets

| fontconfig | here |
| --- | --- |
| `FcCharSetCreate` | `OwnedCharSet::new()` |
| `FcCharSetDestroy` / `Copy` | ordinary ownership |
| `FcCharSetAddChar` | `insert` |
| `FcCharSetHasChar` | `contains` |
| `FcCharSetCount` | `len` |
| `FcCharSetUnion` | `union` |
| `FcCharSetMerge` | `merge` |
| `FcCharSetSubtract` | `subtract` |
| `FcCharSetFirstPage` / `NextPage` | `chars()`, `ranges()` |
| `FcCharSetIsSubset` | — no equivalent; `subtract` and check emptiness |
| `FcCharSetSubtractCount` | — internal; used by scoring, not exposed |

## Languages

| fontconfig | here |
| --- | --- |
| `FcLangSetCreate` | `OwnedLangSet::new()` |
| `FcLangSetAdd` | `insert` |
| `FcLangSetHasLang` | `has_lang` → `LangResult` |
| `FcLangSetCompare` | `compare` |
| `FcLangSetContains` | `contains_set` |
| `FcLangSetUnion` / `Subtract` | `union` / `subtract` |
| `FcLangSetGetLangs` | `langs()` |
| `FcGetLangs` | `langs::LANGS` |
| `FcLangNormalize` | — internal, applied by `default_substitute` |

## Directories and caches

| fontconfig | here |
| --- | --- |
| `FcDirCacheLoad` | `Config::cache_path()` then `Cache::open()` |
| `FcDirCacheLoadFile` | `Cache::open()` |
| `FcDirCacheRead` | `Config::caches(policy)` — the policy decides |
| `FcDirCacheValid` | applied by `CachePolicy`; reported by `Caches::skipped()` |
| `FcDirCacheRescan` | `Builder::dir()` |
| `FcDirCacheUnload` | drop |
| `FcDirScan` | `scan_file()` per file, or `Builder::dir()` |
| `fc-cache` | `Builder::tree()` |
| `FcDirCacheClean` / `Unlink` | — no equivalent |
| `FcDirCacheCreateUUID` / `DeleteUUID` | — read-only: a `.uuid` file is honoured, never written |
| `FcCacheDir` / `NumFont` / `NumSubdir` | `Cache::dir`, `fonts`, `subdirs` |
| `FcAtomic*` | — internal; the builder writes `.NEW` and renames |

Scanning needs the `scan` feature. Everything that only reads works without
it, and without any dependency at all.

## Gaps

Things with no equivalent, separated by why.

**Not written yet.** These would be additive, and are listed because
somebody will want them.

- `FcNameParse` and `FcNameUnparse`. The `:`-separated pattern syntax
  (`"DejaVu Sans:bold:lang=en"`) is parsed inside `examples/fc_match.rs` and
  has never been promoted. It is the format every fontconfig command line and
  a good deal of application code speaks.
- `FcPatternFormat`. `fc-list`-style format strings, likewise implemented in
  the examples.
- Properties under a name the crate does not know. A configuration can invent
  one, and this crate honours that internally — `10-scale-bitmap-fonts.conf`
  depends on it — but `Query::add` takes an `Object`, and the type that names
  an arbitrary property is not public. Reading one back works
  (`Query::custom`); setting one does not.
- `FcFontList`'s object-set filtering. Listing works by walking `caches()`,
  but the deduplication `FcObjectSet` implies is left to the caller.

**Deliberately absent.**

- The current configuration, the rescan interval, and reinitialisation.
  Process-global mutable state, replaced by holding a value.
- `FcBlanks`. Deprecated in fontconfig itself and ignored since 2.11.
- `FcStr*`. String and path helpers that Rust's standard library covers.
- `FcAtomic*`. File locking exposed as public API; used internally here.
- `FcFreeTypeQuery*` and anything taking an `FT_Face`. Scanning goes through
  `read-fonts`, so there is no FreeType handle to accept, and `FcTypeFTFace`
  is a value type this crate rejects — it cannot appear in a cache anyway.

**Cannot be reached from a cache.**

- Reading a cache written for a different byte order. The architecture is in
  the file's name, so a foreign-endian cache is never looked for rather than
  rejected. See the README on format compatibility.
