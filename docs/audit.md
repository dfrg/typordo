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
| 18 | Cache lookup stopped at the first candidate | Test fails on old behaviour | `e2db80c` |
| 20 | Cache stayed current when its directory vanished | Test fails on old behaviour | `e2db80c` |
| 19 | Traversal accepted partially corrupt caches | Test fails on old behaviour | `8e41e5b` |
| 17 | Relocated cache kept the build machine's subdirectory paths | Test fails on old behaviour | `5c3406a` (part) |
| 9.2-9.5 | Range, charset, langset and matrix comparison | 36 cases against `fc-pattern -c` | `31575bb` |
| 6 | Conditional `<alias>` tests discarded | DIFF->MATCH against `fc-pattern -c` | `31575bb` |
| 7 | Empty selector patterns inverted accept/reject | DIFF->MATCH against `fc-list` | `7d16166` |
| 8 | `prgname`, `desktop` and `order` never set | Both agree against `fc-pattern -c -d` | `702677d` |
| 3.2 | `ignore_missing` never read; a missing include was silent | Test fails on old behaviour | `207a0fe` |
| 10 | `<const>` was case-sensitive, and an unknown one poisoned | DIFF->MATCH against `fc-list` | `f3245a4` |
| 11 | Element text was trimmed; no CDATA, no numeric references | DIFF->MATCH against `fc-list` | *this commit* |
| 12 | Values were stored against properties that cannot hold them | DIFF->MATCH against `fc-list` | *this commit* |
| 2 | No startup fallback when the configuration will not load | DIFF->MATCH against `fc-list` | *this commit* |
| 22 | Language comparison ignored country sets and extra strings | Tests fail on old behaviour | *this commit* |
| 21 | Cache rebuilds took no inter-process lock | Tests fail on old behaviour | *this commit* |

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

**19 — a cache damaged past its header.** `FcDirCacheMapFd` runs
`FcCacheOffsetsValid` on every map, unconditionally, and refuses the file
entire. Here the header was checked on open, the full walk was a separate
call the traversal never made, and the iterators quietly skipped records that
did not hold up -- yielding a partial font set from a cache fontconfig would
have rejected, and pruning whatever subdirectory tree hung below the skipped
record. The walk now runs before a cache is handed out, and a failing one is
passed over like any other candidate, so a good one further down still gets
its chance.

This one has a price, and it is the entry worth reading twice. Load went from
1.47x fontconfig to **0.77x** the moment the deep check went in. Most of that
was doing more than fontconfig does: `value_at` decodes every value, which for
a string means scanning for a terminator and validating UTF-8, while
`FcCacheOffsetsValid` checks the type tag and that an indirect offset lands
inside the file. A structural-only `value::check_at` brings it to **0.85x** --
slower than not checking, and about what fontconfig costs, which is the
comparison that means anything, since it now does the same work. The earlier
figure was measuring the absence of a safety check.

**17 — a cache that has moved, in part.** `FcConfigAddCache` compares the
directory a cache records with the one it was asked for, and when they differ
rewrites two things under the requested directory: every subdirectory in the
cache, and every font's `FC_FILE`. This crate did neither.

The subdirectory half is fixed. It is the more damaging one -- a wrong
subdirectory is not merely a wrong string, it sends the walk into a tree that
does not exist and silently drops every font below it -- and it is entirely
internal to the walk, so it could go in without deciding anything.

The font-path half is **set aside for a decision**, and the reason is the
design rather than the work. A `PatternRef` is a cursor into the mapped cache;
there is nowhere to put a rewritten path without either owning the pattern or
handing the caller a helper it must remember to call, and a helper you must
remember is the same silent wrongness in a new place. It wants an answer about
what `Caches` yields, which is a public shape. Noted for the author.

Worth saying that relocation is not exotic. The copy that causes it -- `tar
-p`, `rsync -a`, `mv`, a sysroot image -- generally preserves directory
timestamps, so the relocated cache reads as perfectly current in its new home,
which is exactly when nothing warns you.

**9.2-9.5 - comparison.** `FcConfigCompareValue` dispatches on type, and
this crate handled three of the eight. Charsets and language sets had no arm
at all, so every `<test>` against one answered false. Two ranges had no arm.
A number against a range was handled, and backwards.

That last one is the reason this entry has a harness attached rather than a
test. `contains` asks whether the **left** falls inside the right, so a font
whose `size` spans `[10,20]` does *not* contain `12` -- the number, promoted
to the point range `[12,12]`, contains the span or it does not. This crate
computed "is the number inside the range" whichever side the range was on,
which is the reading the operator's name suggests and not the one fontconfig
implements.

The missing piece underneath was `FcConfigPromote`: when the two sides differ
in type, each is converted towards the other before anything is compared. A
number becomes a one-point range, an absent value becomes the identity matrix
or an empty set, and a **string becomes the language set naming it** -- which
is how `<test name="lang">ja</test>` reaches a font's language list at all.
Where promotion cannot bring them together, `not_eq` and `not_contains` are
satisfied and everything else is not.

`scripts/compare_parity.sh` is new and is the point of the entry. The other
harnesses drive whole queries through real fonts, and a font set reaches only
the comparisons its fonts provoke: nothing in a normal corpus carries a
charset test or a range on both sides, so these went unchecked while three
were wrong -- the same shape as the scanner findings above. It asks
`fc-pattern -c` and this crate the same 36 questions, one operator at a time.

Building it turned up a further gap the audit did not name. Objects had no
declared type, so the example's query parser guessed from the text: `:size=[10
20]` became a string, and `:scalable=True` became the family name "True",
since only the lowercase spelling was recognised. Fontconfig converts by the
object's declared type (`FcNameConvert`), and this crate's own documentation
had been naming those types all along -- every variant's doc comment says what
it holds. `Object::value_type` now says it in code, and a test reads the
comments back to keep the two from drifting.

**6 - conditional aliases.** An `<alias>` may carry `<test>` elements, and
`FcParseAlias` puts them ahead of the family test it synthesizes. They were
parsed, attached to the alias frame, and then dropped on the floor. The effect
is worse than ignoring the alias: a `<test name="lang">ja</test>` alias
applied to every language. Verified in both directions -- a ja-only alias
fired for `serif:lang=de` before the fix and does not after.

**7 - empty selector patterns.** `FcListPatternMatchAny` walks a selector's
elements looking for one that disagrees, and an empty selector has none, so it
returns true: an empty `<pattern/>` matches every font. That makes an empty
`<rejectfont>` reject the lot. This crate skipped empty patterns at parse
time, so the rule did the opposite of what it says -- not a weaker filter, an
inverted one.

The same function skips `namelang` by name, because that property sets
`familylang`, `stylelang` and `fullnamelang` together and never appears on a
font: testing for it would fail every selector that mentions it. We tested for
it and failed. Both now agree with `fc-list` on the whole corpus -- 0 files for
an empty reject, 2385 for an empty accept, and 0 for a reject on `namelang`
alone, which reduces to the empty case once the element is skipped.

**8 - the properties a configuration tests.** `FcDefaultSubstitute` ends by
adding `prgname` (the executable's basename), `desktop` (from
`XDG_CURRENT_DESKTOP`, empty treated as absent) and `order` (0). None is
scored against, which is why their absence showed up in no harness: they exist
so a configuration can test them. A distribution's `<test name="prgname">`
rule -- the usual way a terminal is kept off a proportional font -- could not
fire here, because the property it tests was never set.

`prgname` is added twice upstream, and the repetition matters:
`FcConfigSubstituteWithPat` adds it before any pattern rule runs, so waiting
for the defaults would leave exactly those rules looking at a pattern without
it. `desktop` and `order` are not added there, and are not here either.

Checked by giving both implementations the same rule keyed on their own
executable name: `fc-pattern` fires the `fc-pattern` rule and this crate fires
its own, each reporting the name it should. `order: 0` matches, and `desktop`
appears in both or neither as `XDG_CURRENT_DESKTOP` is set or empty.

**3.1 - include resolution.** Read against both trees and found already
correct. `FcConfigGetFilename` sends a `~` path to the home directory, an
absolute path straight through, an `xdg` prefix to `XDG_CONFIG_HOME`, and
anything else to each `FONTCONFIG_PATH` entry in turn and then the built-in
configuration directory -- **not** to the including file's own directory,
which is the plausible wrong answer. That is what `include_paths` does.

**3.2 - `ignore_missing`.** The attribute was never read, and a missing
`<include>` was passed over in silence. Fontconfig prints `Cannot load config
file` and loads everything else, so the font list is the same either way --
which is the whole problem. An include naming a path that has moved goes on
contributing nothing, and nothing tells you.

`Config::warnings` now reports it, the same bargain `Caches::skipped` already
makes. It is not an error and does not fail the load, because upstream does
not fail either: `fc-list` returns all 2385 fonts with a missing include and
prints the complaint to stderr.

Reading `ignore_missing` meant reading `FcNameBool`, where the first letter
decides -- `yes`, `on`, `1` and `True` are all true -- and that turned up a
gap beside it: `<bool>` elements accepted only the literal `true` and `false`,
so `<bool>yes</bool>` in a selector was discarded. Worse, a spelling
fontconfig cannot read is **false** to it, not ignored, so `<bool>bogus</bool>`
selects the non-scalable fonts; treating it as unusable left 18 more fonts in
the list than fontconfig leaves. Ten spellings are now compared against
`fc-list` in `select_parity`, and all ten agree.

**10 - `<const>`.** The table itself was complete and in the right order, and
the reason the order matters was already written down. Two things around it
were wrong.

`FcNameGetConstant` compares with `FcStrCmpIgnoreCase`, so `<const>Bold</const>`
resolves exactly as `<const>bold</const>` does. Ours compared exactly, and the
capitalised spelling silently resolved to nothing.

Which led to the second, and the more interesting one. A name the table does
not hold is `FcTypeVoid`, and `FcParsePatelt` stops at the first Void value it
pops -- so the `<patelt>` adds *nothing to the pattern*, and the elements
beside it still apply. This crate treated it as unevaluable and poisoned the
whole selector, on the principle that narrowing a selector to the half we
understood would reject fonts fontconfig keeps. The principle is right and
still holds for the cases it was written for; an unknown `<const>` is not one
of them, because fontconfig does not fail to evaluate it -- it evaluates it to
nothing.

`fc-list` settles it: a selector of `family + unknown const` keeps exactly as
many fonts as `family` alone. A test asserting the opposite has been corrected;
it was encoding this crate's guess, not upstream's behaviour, which is the
failure mode worth naming -- a test can pin a mistake as firmly as it pins a
fix.

**11 - character data.** Three things, one of them a real change of
behaviour.

Fontconfig hands each parse function the element's buffer exactly as it
accumulated. It does not trim. So `<dir>` written across three lines with an
indent names a directory that does not exist, `<bool>  true  </bool>` is not a
boolean -- `FcNameBool` looks at the first character and finds a space -- and
`<const>  bold  </const>` resolves to nothing. This crate trimmed, in two
places, and so accepted configurations fontconfig rejects. That is a
difference in which fonts exist, not a kindness. Measured both ways: a padded
`<dir>` yields no fonts in either implementation now, and each of the padded
`<int>`, `<bool>` and `<const>` fails in its own distinct way, which is what
makes them worth comparing one at a time.

The reader kept a deliberate deviation and it is now written down: a text run
that is *entirely* whitespace is dropped rather than reported. Fontconfig
keeps it in a buffer it then ignores for every element that has children, so
the only value this changes is one written as nothing but spaces -- and
dropping it is what keeps indentation out of every enclosing element's text.

CDATA was silently producing nothing, which is worse than the module's own
promise to "report an error rather than guess". Numeric character references
were left as written. Both work now, in both spellings, with a reference that
leads nowhere still left alone the way any unrecognised entity is.

**12 - values a property cannot hold.** `FcPatternObjectAddWithBinding`
refuses a value whose type the object does not accept, and refuses
`FcTypeVoid` outright. The rule is not simply "the declared type": a number
goes into either numeric property and into a range, and a string goes into a
language set, because those are conversions matching performs anyway.

The two paths differ in how loudly they fail, and both were measured rather
than guessed. In an `<edit>` the value is dropped and the rest of the rule
still runs, so `<edit name="family"><int>1</int></edit>` has no effect. In a
`<patelt>` the refusal is reported at `FcSevereError`, which fails the
**entire configuration**: with a `<dir>` naming one font, a config carrying
`<patelt name="family"><int>1</int></patelt>` makes `fc-list` report the
whole system's fonts, because it fell back to the defaults.

Beside it, a gap that had been there all along: a selector's plain string
compared against a font's language set matched nothing, because there was no
arm for the pair. It is `FcConfigPromote` again -- the string becomes the set
naming that language -- and `<patelt name="lang"><string>ja</string></patelt>`
now rejects the 38 fonts it should.

**2 - the startup fallback.** `FcInitLoadOwnConfig` does not give up when the
configuration will not load. It builds `FcInitFallbackConfig` -- a fixed
document naming the default font directories, the default cache directories
and the usual includes, every one of them `ignore_missing` -- and runs on
that. It is why `fc-list` still finds fonts on a machine whose `/etc/fonts` is
broken, and it is what made the `<patelt>` measurement above read 2385 rather
than 0.

`Config::fallback` is that document, and `Config::load` reaches for it on
failure. The library still hands the error back rather than swallowing it; the
example programs fall back the way `fc-list` does, which is what makes a
comparison against them meaningful in exactly the case where a configuration
is at fault.

`FcInitLoadOwnConfig` also supplies `FC_CACHEDIR` and the XDG cache directory
when the loaded configuration named none, warning unless `FONTCONFIG_FILE` or
`FONTCONFIG_PATH` says the caller meant it. A configuration with nowhere to
put a cache rescans every directory on every run, so this is not cosmetic.

**22 - language sets.** Copying was already right; comparison was not, in two
places, and the *reason* the first one had been left is the interesting part.

`FcLangSetCompare` does not stop at "no language in common". It then asks
whether the two sets name regional variants of one language -- `zh-CN` against
`zh-TW` -- and answers `DifferentTerritory` rather than `DifferentLang` if so.
Without that a Simplified Chinese font scores no closer to a Traditional
Chinese request than a Greek one does. This crate skipped the step, and said
so in a doc comment, on the grounds that "a query built with `Pattern` cannot
carry a langset, so this is only reachable when comparing two fonts".

That was true when it was written and stopped being true earlier in this same
audit: parsing `:lang=en` by the object's declared type puts a langset into
every query that names a language. A documented limitation is only as good as
the assumption under it, and nothing rechecks the assumption when the code
around it moves.

The table upstream generates is derivable from the language list -- group by
what precedes the hyphen, which is all `fc-lang.py` does with it -- so it is
built once from `LANGS` rather than vendored.

The second place: `FcLangSetHasLang` finishes by comparing against the
languages the table cannot name, and ours never looked at them. A set holding
only `en-GB` -- not in the table, so it lives as a string -- answered
"unrelated" to every request, including one for `en-GB` itself.

Worth recording that the obvious expectation here was wrong twice over. `en`
against `en-GB` is `DifferentTerritory`, not `Equal`: `FcLangSetHasLang`
reaches `en` through `FcLangCompare`, which calls the same language in a
different region exactly that. Both tests were written asserting `Equal` and
both were corrected against the source, not the other way round.

**21 - the rebuild lock.** This one was expected to be a design question and
is not; `FcAtomicLock` spells the whole thing out.

Writing was already atomic here -- bytes to a temporary, then a rename -- but
the temporary had a fixed name, `<cache>.NEW`, which is the same name
fontconfig uses. Upstream that is safe because a `<cache>.LCK` beside it
serialises writers; here nothing did, so two processes rebuilding one
directory would write the same temporary file and either could rename the
other's half of it into place. `fc-cache` and a desktop session starting at
once is not hypothetical, it is how a machine boots.

The lock is a hard link: `link` fails when the destination exists, and fails
for every process but one, which is what makes it atomic. Filesystems that
refuse hard links fall back to `mkdir`, atomic for the same reason. A lock
older than ten minutes is assumed to belong to a process that died and is
taken over -- fontconfig's timeout and fontconfig's assumption with it, that
machines sharing a filesystem keep their clocks close enough.

It is released on drop, error paths included, which is the difference between
a failed rebuild and a directory nothing can rebuild for the next ten minutes.

### What fixing 9.2-9.5 broke, and how it showed

Parsing `:lang=en` into a language set -- correctly, as `FcNameParse` does --
put a shape into queries that had never been there before, and one place was
not ready for it. `add_default_langs` decides whether the query already asks
for the locale's language by looking at its `lang` values, and it looked only
at strings. A langset query therefore never counted as asking for its own
language, so the locale's languages were appended beside it.

One extra weak value, and `NotoSans[wght].ttf` moved from second place to
forty-second in `fc-match -a :lang=en`. `sort_parity` and `cover_parity` both
caught it; the unit tests did not, because none of them builds a query the way
a command line does. Fontconfig checks both shapes in the same loop, and now
so does this.

Worth recording as the shape of the risk rather than as a single mistake: the
fix was right, the regression was real, and what found it was the harness that
drives whole queries through real fonts -- the kind of check the new
`compare_parity` deliberately is not.

### Examined and found correct

| # | Finding | What it actually does |
| --- | --- | --- |
| 3.1 | Include resolution | Already matches `FcConfigGetFilename` |

### Version drift, not gaps

| # | Finding | Why it stands |
| --- | --- | --- |
| 23 | Language data trails 2.18.3 (281 vs 339 entries) | The table is generated from 2.17.0 by design |
| 24 | `genericfamily` absent | Confirmed absent from 2.17.0 entirely |

The language table is an assumption about the writer, and `src/langs.rs` says
so. CI demonstrated the *other* direction the first time it ran: on a runner
shipping fontconfig 2.15.0, which has 279 entries, seven fonts differed
because ours knows `got` and theirs cannot. See `docs/gaps.md`.

### Open, with a reason

What is left, and why each is still open rather than fixed.

| # | Finding | Why it is still here |
| --- | --- | --- |
| 13 | Tri-state boolean collapsed | Real, unreachable on any config measured; the fix changes a public type |
| 14 | WOFF and standalone CFF not scanned | Real and confirmed by measurement; blocked on `read-fonts` |
| 17 | Relocated caches keep embedded font paths | Needs a decision about what `Caches` yields |
| 25 | Application-font preference not representable | API design |

**13 - `FcDontCare`.** Fontconfig's booleans have three states, and the third
is not decorative: `FcCompareBool` takes the *font's* value when the pattern
says `FcDontCare`, and scores it as a match either way. `FcNameBool` spells it
`d`, `x`, `2` or `or`, so `<bool>dontcare</bool>` produces it.

Ours has two states, and reads that spelling as `false` -- a different stored
value and a different score. It is a genuine difference. It is also not
reachable on the system this crate is measured against: of the fifty `<bool>`
elements in every configuration file shipped there, forty-one say `false` and
nine say `true`. None says anything else.

The fix means giving `Value::Bool` a third state, which changes a public type
and every match on it. That is the author's call, not one to make while
running through a list.

**14 - WOFF.** This was written down as "expected to end as an argument rather
than a fix", and the prediction was wrong twice over.

First, the reasoning was never tested. It has been now: a valid WOFF wrapper
around a font already in the corpus is read by `fc-query` in full -- family,
style, weight, charset, languages -- and rejected by this crate as "not a font
file". So it is a real gap, not a difference of opinion about what a font is.

Second, and worth keeping: an earlier version of that measurement said the two
agreed. Two things were wrong with it. The WOFF was malformed -- the table
directory was offset by the size of a header I had forgotten -- so *both*
implementations rejected it, for different reasons. And when the file was
fixed, a directory listing appeared to show both finding it, because the two
were sharing a cache directory and this crate was reading the cache
`fc-list` had just written. Neither error would have survived a comparison
that queried the file directly, which is what settled it.

The gap itself stands where it was: `read-fonts` recognises SFNT and
collections, and a WOFF is neither until something decompresses it. Written up
with the measurement as gap 8 in `docs/fontations-gaps.md`.

**17** and **25** are described where they belong -- 17 above, 25 unexamined
beyond the report. Both want an answer about public shape rather than about
fontconfig.

### On predicting which findings will survive

Three were expected to end as arguments. **20** did not -- it was a decision
this crate had written down and defended, and it did not hold up. **21** did
not either; `FcAtomicLock` simply spells out what to do. **14** did not, and
the reasoning behind the prediction turned out never to have been tested at
all.

That is nought for three. Reading a finding is cheap and guessing at it is
worth nothing.
