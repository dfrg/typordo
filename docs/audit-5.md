# Audit 5 — the first audit, revalidated

A re-run of all 25 findings in [audit-1.md](audit-1.md) against `7d88172`,
comparing source against fontconfig **2.18.3** rather than the 2.17.0 this
crate targets. Sixteen were confirmed fixed. The rest are below, and the
version gap does most of the work in sorting them: two of the residuals turn
out to be things 2.18.3 changed and 2.17.0 does not do, and two more are
version drift this crate had already written down.

| # | Area | Finding | Status |
| --- | --- | --- | --- |
| 2b | config | Only the first `FONTCONFIG_PATH` entry was searched | Fixed, `aff0449` |
| 3b | config | Every include candidate was read; a missing one only warned | Fixed, `aff0449` |
| 10b | config | `<const>` resolution, said to need the property's context | Not a 2.17.0 config difference -- but see below, `7976cce` |
| 11b | config | A malformed value poisoned a selector rather than failing the load | Fixed, `aff0449` |
| 12b | pattern | `PartialEq` is structural; insertion validates nothing | Fixed, `aff0449` |
| 22b | langset | Names outside the table are dropped by copy and ignored by equality | Fixed, `aff0449` |
| 23b | data | The language table is generated from 2.17.0 | Version drift, below |
| 24b | objects | `genericfamily` is a 2.18 property | Version drift, below |
| 25b | api | Application-font preference | Confirmed: the original finding was wrong |

## 2b — a search path is a path, not its first entry

`FcConfigGetFilename` loops:

```c
path = FcConfigGetPath();
for (p = path; *p; p++) {
    file = FcConfigFileExists (s, url);
    if (file)
        break;
}
```

This crate took `config_path().into_iter().next()` and joined the name onto it.
With `FONTCONFIG_PATH=/missing:/etc/fonts` it then found nothing, fell back to
the built-in configuration, and answered every query from a different font set
than fontconfig -- which loads `/etc/fonts/fonts.conf` and gets on with it.

Resolution is now `resolve_config_path`, split out from the environment it
normally reads so it can be tested without setting a variable the whole
process shares, and it handles the two other branches upstream has: an
absolute name taken as it stands, and a `~`-relative one resolved against the
home directory rather than against the path.

## 3b — one include, and a missing one is fatal

Three differences in the same twenty lines, all confirmed against 2.17.0.

**Every candidate was read.** `_FcConfigParse` resolves the name through
`FcConfigGetFilename`, which returns **one** file. Reading every directory on
the search path that happens to hold a file of that name merges configurations
fontconfig would never merge -- and a distribution really does ship the same
name under more than one entry.

**A missing required include is fatal.** `FcParseInclude` calls
`_FcConfigParse (..., complain = !ignore_missing, ...)` and sets
`parse->error` when it fails. A severe error fails the whole load, the
including file's rules with it, and fontconfig runs on its built-in
configuration instead. So one missing file changes every answer, rather than
quietly dropping one file's rules -- which is what this crate did, with a
warning.

**`ignore_missing` covers more than missing.** `_FcConfigParse` ends with

```c
if (!complain) {
    FcStrBufDestroy (&reason);
    return FcTrue;
}
```

so the attribute suppresses a file that is present and will not parse, just as
it suppresses one that is absent. This crate still propagated the read error.

A fourth came out of testing it: `<include prefix="relative">` is not a thing.
`FcParseInclude` reads exactly one prefix, `xdg`, and ignores every other
value -- `relative` included, though the *other* path elements do honour it
through `_get_real_paths_from_prefix`. This crate honoured it for includes as
well, so a configuration written that way loaded here and failed in
fontconfig. No shipped configuration on this system uses it; the test fixtures
here did, and now name a search path instead, which is also how a
configuration tree outside `/etc/fonts` has to be loaded for fontconfig to
read it.

`include_parity` is new and compares the one thing none of the field
comparisons can see: whether the configuration under test loaded at all, or
the built-in fallback did. Eleven configurations, five of which fail against
the previous code.

## 10b — `<const>` is object-aware in 2.18.3 and not in 2.17.0

The finding is that a `<const>` should resolve for the property it is being
used with -- `<patelt name="width"><const>normal</const></patelt>` meaning 100,
the width constant, rather than 80, the weight one that is declared first.

That is 2.18.3's behaviour. In 2.17.0 both config paths take the name-only
lookup:

- a `<patelt>` value goes through `FcPopValue`, whose `FcVStackConstant` case
  calls `FcNameConstant`, which is `FcNameGetConstant`, which returns the
  first entry with that name;
- a `<const>` in a rule goes through `FcConfigEvaluate`'s `FcOpConst` case,
  which calls `FcNameConstant` as well.

`FcNameConstantWithObjectCheck` exists in 2.17.0 but is reached from exactly
one place: `FcNameConvert`, and so only from `FcNameParse`. So the object-aware
resolution the finding asks for is real, and it belongs to **name strings**,
not to configurations.

Which turned out to matter, because this crate did not do it there at all --
see 11b's neighbour below. `:weight=bold` reached matching as the string
`"bold"` where fontconfig has the number 200, `:bold` on its own was dropped,
and no constant was resolved in a name anywhere. That is fixed, along with
`_` as a separator, charsets, matrices, and constants written as range bounds.
`parse_parity` is new and compares 72 names across 28 objects; it had no
predecessor, which is why a wrong constant reached every other harness as a
query neither side disagreed about.

The config behaviour is left as it is, and the comment in `constant` saying
that `<patelt name="width"><const>normal</const></patelt>` resolves to 80 is
correct for the targeted version.

## 11b — a malformed value fails the file, and the severities are graded

The finding is that an unreadable value poisoned the selector holding it where
fontconfig rejects the whole configuration. It does, and the more useful
correction is where the severity lives: it belongs to the **element's parser**,
not to what the value was going to be used for. `FcParseInt` is the same
function whether the `<int>` is in a `<patelt>`, a `<test>` or an `<edit>`, so
this crate's split between "poison the selector" and "salvage the rule" had no
counterpart upstream and is gone.

What is graded, and it is not obvious from the outside:

| written | fontconfig | why |
| --- | --- | --- |
| `<int>notanumber</int>` | **fails the file** | `strtol` must consume all of it |
| `<int>  200  </int>` | **fails the file** | trailing space is not consumed |
| `<int></int>` | 0 | `strtol` consumes nothing, and nothing is all of it |
| `<double>abc</double>` | **fails the file** | as above, with `strtod` |
| `<range>` with one or three bounds | **fails the file** | `invalid range` |
| `<matrix>` with two values | ignored, warning | `FcParseMatrix` warns and pushes nothing |
| `<matrix>` with five | **fails the file** | one left on the stack is severe |
| `<charset>` holding a `<string>` | **fails the file** | `invalid element in charset` |
| `<charset>` with a codepoint out of range | kept, warning | the element keeps the rest |
| `<charset></charset>` | contributes nothing | pushed only if it took a character |
| `<langset>` holding an `<int>` | **fails the file** | `invalid element in langset` |
| `<langset>ja</langset>` | contributes nothing | bare text is not a `<string>` child |
| `<bool>bogus</bool>` | false, warning | `FcConfigLexBool` accepts any word |
| `<const>nosuchname</const>` | `FcTypeVoid`, warning | and the `<patelt>` stops there |

The last few rows are the ones with teeth. A value that contributes *nothing*
is not the same as one that cannot be evaluated: `FcParsePatelt` stops at the
first `FcTypeVoid`, so a `<patelt>` holding an empty `<charset>` adds no
element, and the selector is whatever elements came before it. This crate
treated those as unevaluable and refused to match at all, which kept fonts
fontconfig rejects -- measured, and one of its own test fixtures asserted the
wrong answer.

`malformed_parity` is new: 29 configurations, comparing the one thing that
distinguishes these -- whether the file loaded -- rather than the resulting
font list, since two implementations that refuse a file then fall back to
different things. Eleven of the 29 fail against the previous code.

## 12b — equality, and values a property cannot hold

Two halves, and they want opposite answers.

**Insertion.** `FcPatternObjectAddWithBinding` refuses `FcTypeVoid` outright
and then refuses anything `FcObjectValidType` rejects, warns, and returns
false. `Pattern::add` accepted both. That is not tidiness: a pattern carrying a
string where a number belongs scores as a type mismatch against every font, so
accepting one quietly turns a query into one that matches nothing. It now
drops them, and `Object::accepts` is the same check for a caller who would
rather ask first.

**Equality.** `FcPatternEqual` compares through `FcValueEqual`, which promotes
an integer to a double, folds case on strings, and ignores bindings entirely.
Ranges are the strange one: `FcValueEqual (a, b)` is `FcRangeIsInRange (a, b)`,
which is `a` **inside** `b` -- so `[50 100]` equals `[0 200]` and `[0 200]`
does not equal `[50 100]`. Equality that is not symmetric is a good reason on
its own not to spell this `==`.

That is not what `==` should mean for a Rust type. Derived `PartialEq` answers
"did this round-trip", which is what the cache tests need and what a reader
expects of `==`; a case-insensitive `==` that also calls `Int(200)` and
`Double(200.0)` equal would be a trap, and would not survive contact with
`Hash`. So the derived one stays and `Pattern::equivalent` is the fontconfig
question, named so that choosing it is deliberate. `Value::equivalent` is
`FcValueEqual` underneath it.

### What the insertion half turned up

`FcPatternAdd`'s check is not the only one, and the other one is graded
differently. An `<edit>` goes through `FcConfigValues`, which drops a
`FcTypeVoid` from the list one value at a time, and then through
`FcConfigAdd`, which walks what is left and adds **none** of it if *any* of it
is a type the property cannot hold. So an edit giving `weight` a string and a
number stores neither, where dropping the string on its own would have stored
the number.

And the delete is not conditional on the add:

```c
case FcOpAssign:
    if (value[object]) {
        FcConfigAdd (&elt[object]->values, thisValue, FcTrue, l, ...);
        if (thisValue)
            FcConfigDel (&elt[object]->values, thisValue, object, table);
```

`FcConfigAdd` returns false and `FcConfigDel` runs anyway, so an assign whose
values are unusable does not leave the property alone -- it **empties** it.
`FcOpAssignReplace` is the same, deleting everything before it tries. Measured
against 2.17.1: `<edit name="weight" mode="assign"><string>x</string></edit>`
on a pattern with `weight=200` leaves no weight at all.

Eight `compare_parity` cases, three of which fail against the previous code.

### A harness that was comparing two different things

Writing those cases turned up a flaw in `compare_parity` itself: it asked
fontconfig for `fc-pattern -c` and this crate for a fully substituted query.
`-c` is *config* substitution alone -- `-d` is what runs
`FcDefaultSubstitute` -- so any property the defaults fill in was being
compared against one that had not been through them. It never mattered,
because until now every case compared `family` or `style`, which the defaults
do not touch. The weight cases compared a property they do, and it showed up
immediately as a difference that was not one. Both sides now do both passes.

## 22b — a language the table cannot name is still in the set

`LangSet` keeps names outside fontconfig's table -- `en-GB`, say -- as strings
alongside the bitmap, which `FcLangSet` does too. `FcLangSetCopy` copies them
and `FcLangSetEqual` compares them as a set:

```c
if (!lsa->extra && !lsb->extra)
    return FcTrue;
if (lsa->extra && lsb->extra)
    return FcStrSetEqual (lsa->extra, lsb->extra);
return FcFalse;
```

Here `from_languages` copied only the bitmap and `PartialEq` compared only the
bitmap, so a set holding nothing but `en-GB` came back empty from a copy and
compared equal to an empty set. `langs`, `len` and `contains` on `AnyLangSet`
had the same hole -- the owned half of it, at least. A *cached* set never has
extras at all: `FcLangSetSerialize` sets the field to `NULL` and says so in a
comment, which is why this only ever separates two scanned sets.

## A note the revalidation made in passing

Its verification run reported three cache tests failing on the machine it ran
on -- `a_changed_directory_is_rescanned` and two like it -- and said,
correctly, that this did not establish a regression: they assume a directory's
timestamp moves after a same-tick change, and on that filesystem it did not.

They now say so themselves. Each asserts that the stamp actually moved before
asserting that the cache went stale, so a filesystem too coarse to express the
change reports *that* rather than "the cache was not stale", which is what
sent the revalidation looking at the cache code. It is not a cache problem: a
stamp that did not move is one fontconfig would not rescan for either.

The property underneath has a test of its own now, in `stamp`, which involves
no clock at all: given a recorded stamp and a directory, is the verdict right.
It checks a match, four kinds of mismatch, and a directory that has been
removed, and it holds on any filesystem.

## 23b, 24b — version drift, already written down

The language table is generated from 2.17.0 and has 281 entries where 2.18.3
declares 339; `genericfamily` is object 56 in 2.18.3 and this crate's table
ends at 55. Both are in [gaps.md](gaps.md) and both are choices rather than
oversights: the crate targets one version and says which.

## 25b — the original finding was wrong

Confirmed by the revalidation rather than fixed: matching takes a caller-ordered
iterator, so chaining application fonts first or system fonts first expresses
either preference, and `tests/app_fonts.rs` demonstrates all of it. There is no
`FcConfigPreferAppFont` toggle because there is no current configuration to
toggle it on.
