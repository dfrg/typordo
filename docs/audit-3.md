# Third audit: 5 findings

Carried out independently, against fontconfig **2.17.1** built from source,
with typordo pinned at `08ebc75`. Deliberately scoped to what the first two
audits and `gaps.md` had not covered, and it went straight into the scanner's
fallback chains — the paths that only run when a font is missing something.

Like the second, it lists what was checked and found sound, which is where
most of its value is: object ids and value types against `fcobjs.h`, the match
priority order and score formula, the weight mapping, spacing classification,
symbol cmap detection, and the structure of `render_prepare`.

## Findings

| # | Area | Finding | Status |
| --- | --- | --- | --- |
| G1 | scanner | SFNT weight has no style-name / style-flag fallback | Fixed, `8db25e6` |
| G2 | scanner | Width ignores style names; invalid `usWidthClass` collapses to 100 | Fixed, `8db25e6` |
| G3 | scanner | Slant fallback reads `head.macStyle`; FreeType prefers `OS/2.fsSelection` | Fixed, `8db25e6` |
| G5 | scanner | `OS/2.version == 0xffff` is not treated as "no OS/2 table" | Fixed, `8db25e6` |
| G4 | prepare | A font's `DontCare` bool is kept instead of adopting the query's value | Fixed, *this commit* |

## G1, G2, G3, G5 — the fallback chains

All four are the same shape: a value has a chain of sources, and every link
past the first only runs for a font that is missing something. A healthy
corpus exercises none of them, which is why all four were wrong while
`scan_parity` was green over 2385 fonts.

- **G1.** Weight came from a named instance's `wght` or `usWeightClass` and
  otherwise defaulted to medium. Upstream then searches the *style* name for a
  weight word (`FcContainsWeight`) and finally reads FreeType's bold flag. A
  font with no `OS/2` calling itself Bold scanned as Medium.
- **G2.** `usWidthClass` outside 1..9 was mapped to normal. Upstream's switch
  has no default, so the value stays unset and the style name gets its turn --
  a font with a zeroed class calling itself Condensed scanned as normal width.
- **G5.** Version `0xffff` is Adobe's "ignore this table" marker and every
  `OS/2` reader upstream guards on it. Weight, width, foundry and the codepage
  ranges all trusted it here.
- **G3** is the interesting one, and the audit's own description of it is not
  quite right.

### G3, and why Terminus is not a counter-example

The audit says FreeType derives the style flags from `OS/2.fsSelection`
"whenever a valid `OS/2` table exists". The condition also requires the font
to have *outlines*:

```c
if ( has_outline == TRUE && face->os2.version != 0xFFFFU )
    /* fsSelection: italic is bit 0, bold is bit 5 */
else
    /* head.macStyle: bold is bit 0, italic is bit 1 */
```

This crate had a comment asserting the opposite -- that fontconfig follows
`macStyle` -- citing Terminus Bold, which sets the italic bit in `fsSelection`
and is not italic. That comment was right about the behaviour and wrong about
the reason: Terminus ships as `.otb`, an OpenType *bitmap* font, whose `glyf`
table is present and zero-length. No outlines, so `macStyle` is read, so the
`fsSelection` italic bit never gets a vote.

Switching to `fsSelection` without that condition regressed nine Terminus
faces immediately, which is how the distinction was found. The scanner already
knew the rule -- `has_glyf = table_len(font, b"glyf") > 0`, with a comment
explaining `.otb` -- but `style_flags` was written with a plain
`has_table` and drifted from it. There is one `has_outlines` now.

`scripts/fallback_parity.sh` is new and builds the fonts these paths need,
since none exists to be found: `scripts/lib/sfnt.py` performs the surgery on a
real font -- dropping `OS/2`, zeroing `usWidthClass`, setting the italic bit
in one table and clearing it in the other -- and both implementations are
asked about the result. 165/165 fields identical over 15 crafted fonts.

## G4 — a font that does not care

`FcCompareBool` is called as `(v1 = pattern, v2 = font)` and sets
`bestValue = v2` only when the font's boolean is not `FcDontCare`; when the
font says `DontCare` the *query's* boolean is what `FcFontRenderPrepare` puts
in the result. This crate took the font's value at the winning index either
way, so a prepared pattern handed a renderer a tri-state where fontconfig
always resolves to the caller's answer.

Reproduced with the audit's own configuration -- a `target="scan"` rule
setting `antialias` to `dontcare` -- and it is exactly the antialias and
hinting knobs that configurations set that way. With no value in the query
both sides report `DontCare`, since then the object is only on the font and
`render_prepare` copies it across untouched.

Fixed alongside F4 of the second audit, which is the same machinery:
`BestValue::resolved` now carries a `Value` rather than a number, so "what the
winning pair resolved to" covers a collapsed range and a resolved `DontCare`
alike.