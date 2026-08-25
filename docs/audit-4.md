# Audit 4 — the second audit, revalidated

A re-run of every finding in [audit-2.md](audit-2.md) against `7d88172`, by an
agent other than the one that fixed them, with fontconfig 2.17.1 built from
source as the oracle. Ten of the thirteen were confirmed fixed at runtime. The
other three are below.

This is the second time a revalidation has caught something marked fixed that
was not, which is the argument for having them.

| # | Area | Finding | Status |
| --- | --- | --- | --- |
| F8b | rules | The test fires correctly, but marks the wrong value | Fixed, pending |
| F9b | cache | Our written caches still tagged values strong | Fixed, pending |
| F13b | scanner | A `ScriptList` at offset zero still yields no `capability` | Won't fix, below |

## F8b — the test fires correctly, and marks the wrong value

The first fix made `<test name="family">Alpha,Zeta</test>` decline to fire for
a query of just `Alpha`, which is what fontconfig does. It did not touch the
other half of what `FcConfigMatchValueList` returns: **which value** matched,
which is where a later match-relative `<edit>` inserts.

The loops nest the other way round from the obvious reading:

```c
while (e) {                              /* the expressions */
    value = FcConfigEvaluate (..., e);
    ...
    for (v = values; v; v = FcValueListNext (v)) {   /* the pattern's values */
        if (FcConfigCompareValue (&v->value, t->op, &value)) {
            if (!ret)
                ret = v;
            if (t->qual != FcQualAll)
                break;
```

So the *first listed value* that matches anything sets the mark, and
`if (!ret)` freezes it against every later expression. Marking the first
**query** value that matched any expression -- which is what this crate did --
lands somewhere else as soon as the two lists are in different orders:

| test | query | fontconfig | was |
| --- | --- | --- | --- |
| `Alpha,Zeta` append `Hit` | `Zeta,Alpha` | `Zeta Alpha Hit` | `Zeta Hit Alpha` |
| `Alpha,Zeta,Beta` assign `Hit` | `Alpha,Beta,Omega` | `Alpha Hit Omega` | `Hit Beta Omega` |
| `SA,SB` append `Hit` on `style` | `SB,SA` | `SB SA Hit` | `SB Hit SA` |

The second row is the family table earning its place: `Zeta` is absent from the
query, so the table check clears the mark `Alpha` had already set, and `Beta`
then sets it again -- one value further along.

Two more differences came out of writing it faithfully rather than patching the
symptom. `qual="first"` and `qual="not_first"` are not part of the scan at all;
upstream runs the ordinary one and then asks whether the mark is the head of
the list:

```c
if (!vl ||
    (r->u.test->qual == FcQualFirst && vl != e->values) ||
    (r->u.test->qual == FcQualNotFirst && vl == e->values))
        goto bail;
```

Reading them as "does value 0 match" and "does any value after 0 match" agrees
only while a test lists one value and the query does not repeat it. A
`not_first` test against a query holding the same style twice fired here and
does not in fontconfig; a `first` test listing two values fires here on a match
the mark never reached.

`compare_parity` gained eight cases covering the mark and the two qualifiers.
Five of them fail against the previous code.

## F9b — a cache of ours still said strong

The tag was fixed in both directions and the rebinding after a match was
added, and both were confirmed at runtime. What was left is the writer.
`FcValueListSerialize` copies the value and the next pointer:

```c
vl_serialized->next = NULL;
vl_serialized->value.type = vl->value.type;
switch ((int)vl->value.type) { ... }
```

There is no line for the binding, and the block was allocated zeroed, so
**every value in a cache fontconfig has written is weak**. This crate
serialized the pattern's real binding, and the scanner adds values strongly --
so fontconfig reading our cache reported `capability(s) lang(s)` where reading
its own it reports `(w)`, and matched their contents differently.

The fix is to write nothing: the field is already zero in the reserved buffer,
which is upstream's mechanism as well as its result. Bindings do not survive a
cache for either implementation, and nothing needs them to --
`FcCompareValueList` reads them off the *query*, and `FcFontSetMatchInternal`
rewrites the matched font's from the scores before anything looks at them.

The unit test that covered this asserted a round trip, which agreed with
itself whichever way round the encoding was; it now asserts that everything
comes back weak. `write_parity` compares `fc-match -v`'s binding marks between
a cache we wrote and one `fc-cache` wrote, over the whole corpus: 9 of 9
queries, both rounds.

## F13b — a `ScriptList` at offset zero, deliberately not reproduced

`variabletest_matching.ttf` has `GSUB` with `scriptListOffset = 0`. FreeType's
`GetScriptTags` seeks to `base + 0`, reads the table's `majorVersion` (1) as a
script count and the next word (0) as the one tag, so `addtag` drops the
invalid tag and fontconfig emits `capability=""`. `read-fonts` reads the null
offset as "no script list" and this crate produces no `capability` at all.

Matching it would mean reproducing a byte-level misread of a malformed table:
not fontconfig's semantics but its undefined response to broken input, and a
different FreeType would answer differently. The finding's substance -- a
script list yielding only invalid tags must still produce an empty
`capability` rather than none -- was fixed in `f887f9c` and is tested with a
well-formed list carrying one broken tag.

This is the second entry in [gaps.md](gaps.md) of its kind, and the rule is
the same both times: reproduce what fontconfig means, not what it does when
it reads past the end of something.
