# Second audit: 13 findings

Carried out independently, like the first, and sharper: it pinned typordo at
`dedb888`, compared against fontconfig **2.17.1** built from source, and
reproduced every finding at runtime rather than reading for them. It also
carries a "not found to diverge" section, which is worth as much as the
findings — it says where somebody looked and found nothing.

It opened by pointing out that [audit-1.md](audit-1.md) marked a finding fixed
that was not. That was right, and it is the most useful thing either audit has
produced.

## Findings

| # | Area | Finding | Status |
| --- | --- | --- | --- |
| F1 | rules | `<times>` on matrices missing — synthetic oblique gets no `matrix` | Fixed, `740423c` |
| F7 | rules | `<name>` in an edit yields every value; upstream yields the first | Fixed, `740423c` |
| F11 | rules | Arithmetic result type: upstream collapses integral doubles to Integer | Fixed, `740423c` |
| F2 | prepare | Localized family/style/fullname never promoted for the requested language | Fixed, `7fcc614` |
| F3 | scanner | `size` never produced (no `opsz` axis, no OS/2 v5 optical range) | Fixed, `efbeb00` |
| F4 | matching | Range resolution uses the first query value, not the winning one | Fixed, `e080b89` |
| F5 | scanner | Named-instance weight/width ignore the OS/2 × (instance/default) multiplier | Fixed, `efbeb00` |
| F6 | scanner | Missing name fallbacks: `Regular` style, family from the filename, PS-name sanitisation | Fixed, `b230fd6` |
| F8 | rules | Multi-valued `<test name="family">` has different semantics | Fixed, `0eb5534` |
| F9 | cache, matching | Binding encoding inverted, cache values read strong, and no rebinding after a match | Fixed, `08eade1` |
| F10 | prepare | `fontvariations` number formatting / weight rounding differs | Fixed, `e080b89` |
| F12 | rules | Edit marks tracked by index, not by value node | Fixed, `0eb5534` |
| F13 | scanner | Empty `capability` string vs absent element | Fixed, `f887f9c` |

## F1, F7, F11 — matrix multiplication, and what it took

F1 was the correction. The first audit's 9.5 was matrix *multiplication*, and
this log's predecessor folded 9.2 through 9.5 into one row about
`FcConfigCompareValue`, which is comparison. `apply_binary` still called
`as_number` on both operands, so a matrix made the whole expression evaluate to
nothing.

Not academic: stock `90-synthetic.conf` shears a face with no italic of its own
using `<times><name>matrix</name><matrix>…</matrix></times>`, and the rest of
that rule fired here — so such a family was reported oblique and rendered
upright.

Three things were needed, not one:

- `<times>` on two matrices is `FcMatrixMultiply`;
- `apply_binary` never promoted its operands. `FcConfigEvaluate` promotes both
  and dispatches on the type they share, which is what turns the absent matrix
  into the identity — without it the `<name>matrix</name>` half still failed;
- `Expr::Field` yielded *no* values for an absent property, where
  `FcPatternObjectGet (p, object, 0, &v)` yields `FcTypeVoid`. Nothing to
  promote is not the same as Void.

That third point is F7's other half: index zero means one value however many
the property holds, so `<edit name="fullname"><name>family</name></edit>` on
`Alpha,Beta` assigns `Alpha`.

F11 came with the rewrite. `FcConfigEvaluate` computes in double and collapses
the result to an integer whenever it lands on one — every operator, whatever
the operands were. `fc-pattern` prints `12.5 * 2` as `25(i)` and `4 / 2` as
`2(i)`. A test asserting `Double(3.0)` for `24 / 8` was pinning the old
behaviour and now asserts `Int(3)`.

Ten cases in `compare_parity` cover edit expressions, which no harness reached
before: four operators over integral and non-integral results, and four matrix
cases including both operand orders of the synthetic-oblique shear. They
compare the value *and its type*, since that is half of what F11 is about.

## F4, F10 — the winning pair, and how its number is written

`FcCompareValueList` keeps the `bestValue` produced by the pair that won, and
`best_value` here computed it from `wanted.first()` instead. The two only
differ when a query carries several values for one property, which is why no
harness saw it: `weight=300,150` against a variable font is answered `150` by
fontconfig and was answered `205` here. Reversing the query to `150,300` made
the two agree, since then the first value *is* the winner.

`BestValue::resolved` is a `Value` now rather than an `f64`, which is what let
G4 go in beside it: a range resolves to a number and a `DontCare` resolves to
the query's boolean, and both are "what the winning pair produced" rather than
"the font's value as it stands".

F10 is two things in one line. `FcWeightToOpenType` takes an `int` and returns
one, so the fontconfig weight is truncated going in and the axis value rounded
coming out -- weight 150 maps to 562.5 and is written `wght=563`, where this
crate wrote `562.5`. And the number is written by `%g`, which is six
*significant* digits with an exponent outside a readable range, not six
decimal places: `13.33333` prints as `13.3333` and `1234567` as `1.23457e+06`.

The `%g` cases are unit-tested rather than compared against a font, because no
font carries a value that reaches them.

## F2 — the name a non-English desktop sees

`FcFontRenderPrepare` compares the *pattern's* `familylang` against the
*font's* with the **lang** matcher, and moves the winning name and its language
to the front. It reaches that matcher through
`FcObjectToMatcher (object, include_lang = FcTrue)`, which maps `familylang`,
`stylelang` and `fullnamelang` onto `lang` — they have no comparison of their
own. `FcTrue` is passed at exactly one call site, and it is the one that
computes a best value.

Here `best_value` started with `matcher(object)?`, no such mapping existed, so
the promotion never happened and the font's names were copied in cache order.
Source Han Sans JP asked for with `familylang=ja` reported
`Source Han Sans JP` where fontconfig reports `源ノ角ゴシック JP` — the value
most clients display, on every non-English desktop.

Only the *comparison* is borrowed, which is worth stating because it is the
easy thing to get wrong: reading `lang`'s values here would compare the
languages the font can write against the languages its names are written in.

A binding detail came with it, and it is F9's third point. Upstream prepends
the winning name with `l1->binding` — the font's own — and marks only the
*language* strong. This crate forced position 0 strong on both lists. Saying
the name is strong claims the font insists on it; the query does.

`prepare_parity` gained the runs that would have caught this: a font carrying
names in two languages, queried with `familylang=` in four languages and
through `LC_ALL`, compared across all six name and language properties. The
field sweep above could not reach it — it runs under an English locale, so the
name a query would promote is always the one already first.

## F3, F5 — optical size, and what a named instance's weight actually is

Both live in the same forty lines of `fcfreetype.c` and went in together.

**F3.** `size` was never produced at all, from any source. Upstream has four,
in order: a variable face reports the `opsz` axis's whole span as a range; a
named instance reports the coordinate it pins; the default face reports the
axis default; and failing all of those, `OS/2` version 5 carries
`usLowerOpticalPointSize` and `usUpperOpticalPointSize` -- in *twips*, a
twentieth of a point each -- where equal bounds mean a single size rather than
an empty range.

**F5.** A named instance's weight is not its `wght` coordinate. Upstream
computes `mult = coordinate / axis default` and applies it to
`usWeightClass`, so the instance's weight is the *face's* weight scaled by how
far along the axis it sits; the same for width. The two agree only when `OS/2`
agrees with the `fvar` defaults, which is not true of any variable font whose
default master is not Regular. This crate used the axis value directly.

Neither could be checked against the corpus. Of the 2385 fonts this crate is
measured against, **not one declares an optical size** -- `fc-query` reports a
`size` for none of them -- and none has an `OS/2` that disagrees with its
`fvar` defaults. So the fonts were built: `scripts/lib/sfnt.py` gained an
`fvar` writer and a table *inserter* (which has to shift every other table's
offset, unlike replacing one in place), and `fallback_parity.sh` now crafts an
`OS/2` v5 font in both shapes and a three-axis variable font whose
`usWeightClass` is 700 against an `fvar` default of 400.

That last font is the one worth having. It produces five patterns, and every
one of them exercises something different: the default face, three named
instances at both ends and the middle of the axis, and the variable pattern
carrying ranges. 270/270 fields identical over 18 crafted fonts.

## F6 — the names a font does not have

Five fallbacks, all in `FcFreeTypeQueryFaceInternal`, and each is the last
thing standing between a font and being unusable:

- no family from the `name` table, and FreeType has none either -> the
  **file name**, basename with the last extension removed. Without it the font
  has no `family` at all and nothing can ask for it by name.
- no style -> **`Regular`**, with `stylelang=en`. Without it `style=Regular`
  selects nothing.
- no full name -> **family + `" "` + style**, which this crate already did.
- no PostScript name -> the English family with every character PostScript
  will not take in a literal name replaced by a **hyphen**. This crate removed
  whitespace instead, so `Tuffy Two (Test)` became `TuffyTwo(Test)` where
  fontconfig gives `Tuffy-Two--Test-` -- a different name, not just a
  differently-spaced one, and the brackets it leaves in are exactly the
  characters the rule exists to remove.
- the English family is preferred over the first for both of the last two,
  which the full-name path already did and the PostScript path did not.

Four more crafted fonts in `fallback_parity`, and the field list grew to
include the three `*lang` properties, since a fallback that supplies a name
has to supply its language too. 396/396 fields over 22 fonts.

## F13 — an empty capability is not no capability

`FcFontCapabilities` decides once, at the top: a font with no script tags and
no Graphite table returns NULL. Past that it allocates the string and returns
whatever it ends up holding, so a font whose script list contains nothing but
broken tags -- `addtag` skips anything not alphanumeric -- gets an **empty**
capability rather than none.

This crate made the same decision at the top and then undid it at the bottom,
with a `(!out.is_empty()).then_some(out)` that turned the empty string back
into nothing. An element that exists is scored; an absent one is skipped. They
are not interchangeable.

The audit reached this through `variabletest_matching.ttf`, whose `ScriptList`
offset is zero. That case is *not* fixed and deliberately so: with a zero
offset FreeType reads the table header as a script count and invents a tag out
of the bytes that follow, while `read-fonts` parses the same table correctly
and finds nothing. Matching there would mean reproducing a byte-level misread
of malformed input, which is not fontconfig semantics but fontconfig's
undefined response to a broken font -- and a different FreeType would answer
differently. The finding's substance, a script list yielding only invalid
tags, is fixed and tested with a well-formed list carrying one broken tag.

Gating this on `usable_os2` rather than `font.os2()` came with it -- the last
`OS/2` reader still trusting version `0xffff`, missed when G5 went in.

`fallback_parity` gained the `capability` and `properties` fields, the second
of which needs the property *names* rather than a value, since an empty
element and an absent one print identically. 460/460 over 23 crafted fonts.
## F8, F12 — two rules that both hinge on `family` being a list

Neither shows up on a font. Both need a config whose rules are written a
certain way, and no rule in the 1364 files this system ships is written that
way, which is why the parity suite ran clean over them for as long as it did.

**F12.** `<edit>` remembers where it worked so that later edits in the same
`<match>` can carry on from there. Upstream remembers the *value node*: after
`FcOpPrepend`, `elt->values` points at what was just inserted, and a following
`FcOpAssign` replaces that node in place. This crate remembered an *index*, so
prepending shifted the list under the mark and the assignment then landed on
whatever had moved into that slot -- the mark's own value in the simple case,
and something else entirely once two values went in at once.

`prepend Beta` then `assign Gamma` on a query for `Alpha` gave `Gamma`, one
value, where fontconfig gives `Beta Gamma`. The fix keeps the index but shifts
it by however many values the edit inserted ahead of it, which `Edit::apply`
now returns.

**F8.** `<test name="family">Alpha,Zeta</test>` -- a test listing several
families -- runs through a fast path in `FcConfigMatchValueList` rather than
the ordinary comparison. The path walks the *listed* families and, for each
one absent from the pattern, resets the running result. Nothing accumulates:
the **last** family in the list decides the outcome, and an earlier match is
discarded by a later miss.

That is unlikely to be what anyone writing such a rule intends, and it reads
like an upstream accident. It is also what upstream does, so a query for
`Alpha` against `Alpha,Zeta` does not fire, and a query for `Zeta` does. This
crate ran the general comparison, which fires on any overlap, and so fired on
`Alpha`. The guard sits in `Test::evaluate` and is limited to `family` on a
multi-valued test, which is where upstream's own guard sits.

`compare_parity` gained seven cases: three marks with prepends and appends
ahead of them, and four multi-valued tests -- each listed family queried on its
own, both together, and the same list on a non-`family` object to pin that the
fast path is `family`-only. All seven agree.

## F9 — bindings, in three parts, none of which a value comparison can see

A binding says how firmly a pattern holds a value: a **strong** one is the
font's own and will not yield, a **weak** one was contributed by configuration
and gives way. Nothing about the *values* changes, which is why every harness
in this repository agreed on every field while all three of these were wrong.

**The tag was inverted.** `src/write.rs` wrote strong as 0 and weak as 1;
`FcValueBinding` in `fontconfig.h` is the other way round — `FcValueBindingWeak`
is zero. The decoder in `src/value.rs` was inverted to match, so our own caches
round-tripped correctly and the unit test covering exactly that passed. Only a
cache crossing between the two implementations could show it.

**Which made the second part invisible too.** Upstream never serializes this
field: `FcValueListSerialize` copies the value and the next pointer, and the
cache block is allocated zeroed. So *every* value read out of a fontconfig
cache is weak. Reading zero as strong meant we disagreed with fontconfig about
every property of every font on the system — silently, because the values were
right. Confirmed against a real cache: `FcDirCacheLoadFile` followed by
`FcPatternGetWithBinding` reports binding 0 on families that
`FcPatternGetWithBinding` after `FcFontList` reports as 1, because the listing
path re-adds each value and `FcPatternObjectAdd` defaults to strong.

**And matching rebinds the winner.** This is the part with real behaviour
behind it. `FcFontSetMatchInternal` does not return the font it found; it
rebuilds it, and gives each object *one* binding for all of its values:

```c
FcValueBinding binding = FcValueBindingWeak;
if (bestscore[match->strong] < 1000)
    binding = FcValueBindingStrong;
```

1000 is fontconfig's threshold for "this matched exactly" — the value distance
is multiplied by 1000 before the value's position is added in, so anything
under it came from a first-choice value that compared equal. Objects with no
matcher are skipped and keep what they had, which is a larger set than it
sounds: `fullname`, `familylang`, `capability`, `matrix`, `fontvariations` and
the whole rendering group have `NULL` comparators in `fcobjs.h`, so they stay
weak from the cache.

The visible consequence: ask for `DejaVu Sans` and the answer holds `family`
strongly, because that is the name you asked for. Ask for `sans-serif` and the
same font holds it *weakly*, because the name that won was contributed by an
alias. A client that re-substitutes over the result — which is what a binding
is for — gets a different answer in the two cases.

This crate had no equivalent step at all. It is now `Score::binding`, and
`render_prepare` takes the score alongside the font. Upstream materializes a
second pattern to carry the rebinding; passing the score says the same thing
and allocates nothing, and the `Option` distinguishes the two things upstream
does at its two call sites — `FcFontSetMatch` rebinds, a bare
`FcFontRenderPrepare` on a font you did not match does not.

`bind_parity` is new and compares nothing but bindings: 18 queries, 42 objects
each, `fc-match -v`'s `(s)`/`(w)` suffixes against ours. Half of them fail if
the threshold is removed. It covers the first two parts as well, since the
objects with no matcher are the ones whose binding comes straight off the
cache.
