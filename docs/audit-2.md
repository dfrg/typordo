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
| F2 | prepare | Localized family/style/fullname never promoted for the requested language | |
| F3 | scanner | `size` never produced (no `opsz` axis, no OS/2 v5 optical range) | |
| F4 | matching | Range resolution uses the first query value, not the winning one | |
| F5 | scanner | Named-instance weight/width ignore the OS/2 × (instance/default) multiplier | |
| F6 | scanner | Missing name fallbacks: `Regular` style, family from the filename, PS-name sanitisation | |
| F8 | rules | Multi-valued `<test name="family">` has different semantics | |
| F9 | cache | Binding encoding inverted; cache values read Strong where upstream reads Weak | |
| F10 | prepare | `fontvariations` number formatting / weight rounding differs | |
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
