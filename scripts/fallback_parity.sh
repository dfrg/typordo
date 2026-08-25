#!/bin/bash
# Compare the scanner's fallback paths against fc-query.
#
# Weight, width and slant each have a chain of sources, and every link past
# the first only runs for a font that is missing something: no `OS/2`, an
# `OS/2` marked `0xffff`, a `usWidthClass` outside 1..9, an italic bit set in
# one table and not the other. A healthy corpus exercises none of them, which
# is why four of these were wrong while `scan_parity` was green over 2385
# fonts.
#
# So the fonts are built rather than found: `scripts/lib/sfnt.py` performs the
# surgery on a real font, and both implementations are asked about the result.
#
# Run: bash scripts/fallback_parity.sh
set -uo pipefail

FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2; exit 1 ;;
esac
cargo build -q --release --example fc_query || exit 1
QUERY="$CARGO_TARGET_DIR/release/examples/fc_query"
command -v fc-query >/dev/null || { echo "fc-query not found"; exit 1; }

WORK=$(mktemp -d) || exit 1
trap 'rm -rf "$WORK"' EXIT

SOURCE=$(fc-list --format='%{file}\n' | grep -E 'DejaVuSans\.ttf$' | head -1)
[ -n "$SOURCE" ] || SOURCE=$(fc-list --format='%{file}\n' | grep -E '\.ttf$' | head -1)
if [ -z "$SOURCE" ]; then
  echo "fallback parity: no TrueType font to operate on, nothing compared"
  exit 0
fi

python3 - "$WORK" "$SOURCE" <<'PY'
import struct
import sys
sys.path.insert(0, "scripts/lib")
import sfnt

work, source = sys.argv[1], sys.argv[2]
src = open(source, "rb").read()

def named(style, family="Probe"):
    """A complete English name table, so only `style` varies between cases."""
    return [(3, 1, 0x409, 1, family), (3, 1, 0x409, 2, style),
            (3, 1, 0x409, 4, f"{family} {style}"), (3, 1, 0x409, 6, f"{family}-{style}")]

def write(name, data):
    open(f"{work}/{name}.ttf", "wb").write(data)

# Weight: no OS/2 at all, so the style name and then the bold flag decide.
write("weight-no-os2-bold", sfnt.drop_table(sfnt.set_names(src, named("Bold")), "OS/2"))
write("weight-no-os2-black", sfnt.drop_table(sfnt.set_names(src, named("Black")), "OS/2"))
# No OS/2 and a style naming no weight: the macStyle bold bit is all that is
# left, and without it the answer is medium rather than regular.
plain = sfnt.drop_table(sfnt.set_names(src, named("Roman")), "OS/2")
write("weight-no-os2-macstyle-bold", sfnt.patch_u16(plain, "head", sfnt.HEAD_MAC_STYLE, 0x0001))
write("weight-no-os2-macstyle-none", sfnt.patch_u16(plain, "head", sfnt.HEAD_MAC_STYLE, 0x0000))

# Width: `usWidthClass` outside 1..9 is not a width, so the name gets its turn.
for style, tag in [("Condensed", "condensed"), ("SemiExpanded", "semiexpanded"), ("Roman", "none")]:
    d = sfnt.set_names(src, named(style))
    write(f"width-class0-{tag}", sfnt.patch_u16(d, "OS/2", sfnt.OS2_WIDTH_CLASS, 0))
    write(f"width-class99-{tag}", sfnt.patch_u16(d, "OS/2", sfnt.OS2_WIDTH_CLASS, 99))

# Slant: FreeType reads `fsSelection` for a font with outlines and a usable
# `OS/2`, and `head.macStyle` for anything else. The two disagree here on
# purpose, in both directions.
d = sfnt.set_names(src, named("Regular"))
italic_sel = sfnt.patch_u16(d, "OS/2", sfnt.OS2_FS_SELECTION, 0x01)
write("slant-fsselection-italic", sfnt.patch_u16(italic_sel, "head", sfnt.HEAD_MAC_STYLE, 0x0000))
regular_sel = sfnt.patch_u16(d, "OS/2", sfnt.OS2_FS_SELECTION, 0x40)
write("slant-macstyle-italic", sfnt.patch_u16(regular_sel, "head", sfnt.HEAD_MAC_STYLE, 0x0002))
# With no OS/2 there is nothing but macStyle.
no_os2 = sfnt.drop_table(d, "OS/2")
write("slant-no-os2-macstyle-italic", sfnt.patch_u16(no_os2, "head", sfnt.HEAD_MAC_STYLE, 0x0002))
# A name that says so beats either flag.
write("slant-name-oblique", sfnt.patch_u16(sfnt.set_names(src, named("Oblique")),
                                           "head", sfnt.HEAD_MAC_STYLE, 0x0000))

# Version 0xffff means "there is no OS/2 here": weight, width and foundry all
# have to ignore what it says.
d = sfnt.set_names(src, named("Bold"))
d = sfnt.patch_u16(d, "OS/2", sfnt.OS2_WEIGHT_CLASS, 900)
d = sfnt.patch_u16(d, "OS/2", sfnt.OS2_WIDTH_CLASS, 9)
write("os2-version-ffff", sfnt.patch_u16(d, "OS/2", sfnt.OS2_VERSION, 0xFFFF))

# Optical size from `OS/2` version 5, whose two fields are twips. Equal bounds
# mean one size rather than an empty range. No font in the corpus declares an
# optical size at all, so crafting one is the only way to reach the path.
at, length = sfnt.tables(src)["OS/2"]
os2 = bytearray(src[at:at + length])
def version5(lower_twips, upper_twips):
    b = bytearray(os2)
    struct.pack_into(">H", b, 0, 5)
    # v0 is 78 bytes, v1 86, v2-v4 96, v5 100. A short table is not a v5 one.
    b = b[:96] + bytes(96 - len(b[:96]))
    return bytes(b) + struct.pack(">HH", lower_twips, upper_twips)
write("size-os2v5-range", sfnt.replace_table(src, "OS/2", version5(160, 280)))
write("size-os2v5-single", sfnt.replace_table(src, "OS/2", version5(240, 240)))

# A variable font whose `OS/2` disagrees with its `fvar` defaults, which is
# every VF whose default master is not Regular. An instance's weight is the
# face's weight scaled by how far along the axis it sits, not the axis value,
# and the `opsz` axis gives a size to the face, each instance and the variable
# pattern in three different shapes.
d = sfnt.patch_u16(src, "OS/2", sfnt.OS2_WEIGHT_CLASS, 700)
d = sfnt.patch_u16(d, "OS/2", sfnt.OS2_WIDTH_CLASS, 3)
axes = [("wght", 100, 400, 900, 256), ("wdth", 50, 100, 200, 257), ("opsz", 6, 16, 144, 258)]
instances = [(2, [100, 100, 16]), (2, [400, 100, 16]), (2, [900, 100, 16]), (2, [400, 75, 8])]
write("vf-opsz-multiplier", sfnt.add_table(d, "fvar", sfnt.fvar(axes, instances)))

# The name fallbacks, each of which is the last thing standing between a font
# and being unusable: a font that names no family is unmatchable by name, and
# one that names no style cannot be picked by `style=Regular`.
write("names-no-style", sfnt.set_names(src, [(3, 1, 0x409, 1, "Probe Family")]))
write("names-none", sfnt.set_names(src, [(3, 1, 0x409, 5, "Version 1.0")]))
# A family carrying characters PostScript will not take in a literal name.
write("names-psname-chars", sfnt.set_names(src, [(3, 1, 0x409, 1, "Tuffy Two (Test)"),
                                                 (3, 1, 0x409, 2, "Regular")]))
write("names-psname-brackets", sfnt.set_names(src, [(3, 1, 0x409, 1, "A<B>C[D]E{F}G/H"),
                                                    (3, 1, 0x409, 2, "Regular")]))
PY

FIELDS="weight width size slant foundry outline scalable family style fullname
        postscriptname familylang stylelang fullnamelang spacing variable
        namedinstance index"
files=0; total=0; bad=0
for font in "$WORK"/*.ttf; do
  files=$((files + 1))
  differing=0
  for field in $FIELDS; do
    # Several patterns per file for a variable font, so every line counts.
    theirs=$(fc-query --format="%{$field}\n" "$font" 2>&1)
    ours=$("$QUERY" --format "$field" "$font" 2>&1)
    total=$((total + 1))
    if [ "$theirs" != "$ours" ]; then
      bad=$((bad + 1)); differing=$((differing + 1))
      [ "$differing" -le 3 ] && printf '  DIFF %-32s %-14s ours=[%.24s] theirs=[%.24s]\n' \
        "$(basename "$font" .ttf)" "$field" "$ours" "$theirs"
    fi
  done
  [ "$differing" -gt 0 ] && fail
done

echo "fallback parity: $((total - bad))/$total fields identical over $files crafted font(s)"
exit $((FAILURES > 0))
