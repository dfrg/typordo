# Working through an external audit

An audit of this crate against fontconfig was carried out independently, by
agents belonging to someone other than its author, and reported 25 findings.
It pinned typordo at `cf45c33` and compared against fontconfig **2.18.3**,
while this crate targets **2.17.0** deliberately — so some findings are gaps
and some are version drift, and telling those apart is the first thing each
entry does.

This file records what came of each one: what was actually wrong, how it was
checked, and where the fix is. It is kept because a finding marked "fixed" is
worth no more than the evidence behind it, and because the ones marked
"drift" or "disputed" are decisions somebody will want to revisit.

## How each was checked

Nothing here was accepted or dismissed on the report's say-so. Every entry
was read against both source trees, and where a fix went in, it was verified
by making the old behaviour fail: put it back, watch a test or a harness go
red. Where real fonts could settle a question they were used in preference to
synthetic ones — Windows ships symbol and CJK fonts that exist precisely
because these rules do.

## What the corpus could not have found

Three of the scanner findings share a shape worth stating on its own. Of the
2385 fonts this crate is measured against:

- **none** map a character below `U+0020`,
- **none** use a symbol cmap,
- **none** declare exactly one CJK codepage.

Every harness was green while all three were broken. `docs/gaps.md` says one
machine is the whole corpus; this is what that costs. Reading the source
found what measuring could not, and the fixes are verified against fonts the
corpus does not contain.

## Findings

### Fixed

| # | Finding | Verified by | Commit |
| --- | --- | --- | --- |
| 9.1 | Blanks ignored for every string equality | Test fails on old behaviour | `bef9828` |
| 3.3 | `conf.d` accepted any `*.conf` | Test fails on old behaviour | `2b3cdd1` |
| 4 | `<reset-dirs/>` silently ignored | Test fails on old behaviour | `2b3cdd1` |
| 5 | Later `<dir>` could not override an earlier mapping | DIFF→MATCH against real `fc-cache` | `2b3cdd1` |
| 15 | Symbol cmap coverage discarded | 4 real symbol fonts, 12 comparisons | `fe7727d` |
| 16 | OS/2 CJK exclusivity ignored | 145 Windows fonts: 6 differing → 0 | `e6afbdf` |
| 1 | `FONTCONFIG_SYSROOT` ignored | End-to-end against `fc-list` in a built root | `a22d645` |
| 18 | Cache lookup stopped at the first candidate | Test fails on old behaviour | *this commit* |
| 20 | Cache stayed current when its directory vanished | Test fails on old behaviour | *this commit* |

**9.1 — `ignore-blanks`.** `FcConfigCompareValue` uses
`FcStrCmpIgnoreBlanksAndCase` only when `FcOpFlagIgnoreBlanks` is set and
`FcStrCmpIgnoreCase` otherwise, so `"Deja Vu"` and `"DejaVu"` are different
families to a plain `<test>`. This crate ignored blanks for every string
equality. The three places that *do* carry the flag upstream — `FcParseAlias`,
`<selectfont>` matching, and never `contains` — were already right by accident
of the blanket rule.

Worth recording that this one was visible from inside: while building the
family index I noticed every string equality stripped blanks and wrote it
down as a convenience rather than as a difference.

**3.3 — `conf.d`.** `FcConfigParseAndLoadDir` takes only `[0-9]*.conf`. The
numeric prefix is what orders the rules, so a file without one has no defined
place and fontconfig ignores it. A stray `local.conf` was contributing rules
no other implementation could see.

**4 — `<reset-dirs/>`.** The report hedged this as a 2.18 element because the
fontconfig it had to hand was 2.13.1. It is in 2.17.0 — `fcxml.c:420`, calling
`FcConfigResetFontDirs` — so against the version this crate targets it is a
straight gap, more serious than reported.

**5 — duplicate `<dir>`.** `FcConfigAddFontDir` deletes the existing entry for
a source path before inserting, so a later salt or `as-path` replaces an
earlier one. Keeping the first means computing a cache name nothing writes.
`name_parity.sh` gained two cases that build a config, run `fc-cache` and
compare basenames; both DIFF before the fix and MATCH after.

The existing ordering test did not catch this because its fixture declares the
same directory twice *adjacently*, where delete-then-append and keep-first
produce the same list.

**15 — symbol fonts.** A symbol font addresses its glyphs through a Windows
`(3, 0)` cmap. This crate reported `symbol=true` correctly and then walked
only the Unicode subtables, of which such a font has none, so Wingdings
scanned as covering nothing. Fixed by reading the symbol table when there is
no Unicode one, copying `U+F000..F0FF` down to `U+0000`, and emitting an empty
language set — "Symbol fonts don't cover any language, even though they claim
to support Latin1 range", that Latin-1 being the copy.

`scripts/symbol_parity.sh` compares against any symbol font it can find and
reports how many it compared, so a run that finds none says so rather than
passing quietly.

**16 — CJK exclusivity.** When `OS/2` declares exactly one of the four
codepages (`ja`, `zh-cn`, `ko`, `zh-tw`), fontconfig takes the font at its
word and does not derive the other three from coverage. Microsoft YaHei
declares Simplified Chinese and covers enough of Japanese and Traditional
Chinese to satisfy both. The report reproduced this with a font it built; it
did not need to, since Windows ships the fonts the rule exists for.

**1 — `FONTCONFIG_SYSROOT`.** Configuration, fonts and caches are now read
under the root, and paths are recorded as the *target* names them —
fontconfig strips the sysroot back off `FC_FILE` so that a cache built for an
image describes the image rather than the machine that built it. Checked by
building a root containing one font and a config, and confirming `fc-list` and
this crate report the same target-relative path and write caches inside it.

**18 — cache candidates.** `FcDirCacheProcess` walks every configured cache
directory and its loop has no early exit on failure; a candidate that will
not open, or has gone stale, is passed over in the hope of a better one. This
crate took the first file that existed and applied its policy to that alone,
so a system cache left corrupt by an interrupted update took a whole
directory's fonts away from a user cache that was perfectly current. A stale
candidate is now remembered as a fallback, since using one is usually kinder
than losing the directory, but it never shadows a current one.

**20 — a directory that has gone.** This was expected to end as an argument
and did not. `src/stamp.rs` treated a directory it could not stat as leaving
its cache current, reasoning that rebuilding would fail anyway so the cache
was the only description left. It is a description of nothing: the font files
went with the directory, so every path it holds names a file that no longer
opens. Fontconfig drops the directory -- `FcDirCacheProcess` fails on the
stat and so does the rescan behind it -- and it is right to. Reported through
`Caches::skipped` as `DirectoryUnavailable`, which is a different complaint
from a missing or stale cache and worth telling apart.

### Version drift, not gaps

| # | Finding | Why it stands |
| --- | --- | --- |
| 23 | Language data trails 2.18.3 (281 vs 339 entries) | The table is generated from 2.17.0 by design |
| 24 | `genericfamily` absent | Confirmed absent from 2.17.0 entirely |

The language table is an assumption about the writer, and `src/langs.rs` says
so. CI demonstrated the *other* direction the first time it ran: on a runner
shipping fontconfig 2.15.0, which has 279 entries, seven fonts differed
because ours knows `got` and theirs cannot. See `docs/gaps.md`.

### Not yet examined

Read against both trees but not yet acted on. Listed so that "fixed" above
cannot be mistaken for "all of it".

| # | Finding | Priority as reported |
| --- | --- | --- |
| 2 | Root configuration search and startup fallback | High |
| 3.1, 3.2 | Include resolution and `ignore_missing` | High |
| 6 | Conditional `<alias>` tests discarded | Medium-high |
| 7 | Empty selector patterns invert accept/reject | Medium-high |
| 8 | Substitution omits `prgname`, `desktop_name`, `order` | Medium |
| 9.2–9.5 | Range, charset/langset and matrix comparison | High |
| 10 | `<const>` resolution | Medium |
| 11 | XML character data | Low-medium |
| 12 | `Pattern` equality and insertion | Medium |
| 13 | Tri-state boolean collapsed | Medium |
| 14 | WOFF/WOFF2 and standalone CFF not scanned | Medium-high |
| 17 | Relocated caches keep embedded paths | High |
| 19 | Traversal accepts partially corrupt caches | Medium-high |
| 21 | Rebuilds lack an inter-process lock | Medium |
| 22 | LangSet copying, comparison, default insertion | Medium |
| 25 | Application-font preference not representable | API |

**14** is expected to end as an argument rather than a fix: it depends on
`read-fonts`, which recognises only SFNT and collections. See
`docs/fontations-gaps.md`.

**20** was also expected to be an argument, and was not. Predicting which
findings will survive scrutiny is worth less than reading them; that one was
a decision I had written down and defended, and it did not hold up.
