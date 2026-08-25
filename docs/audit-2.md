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
| F3 | scanner | `size` never produced (no `opsz` axis, no OS/2 v5 optical range) | Fixed, *this commit* |
| F4 | matching | Range resolution uses the first query value, not the winning one | Fixed, `e080b89` |
| F5 | scanner | Named-instance weight/width ignore the OS/2 × (instance/default) multiplier | Fixed, *this commit* |
| F6 | scanner | Missing name fallbacks: `Regular` style, family from the filename, PS-name sanitisation | |
| F8 | rules | Multi-valued `<test name="family">` has different semantics | |
| F9 | cache | Binding encoding inverted; cache values read Strong where upstream reads Weak | |
| F10 | prepare | `fontvariations` number formatting / weight rounding differs | Fixed, `e080b89` |
| F12 | rules | Edit marks tracked by index, not by value node | |
| F13 | scanner | Empty `capability` string vs absent element | |

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