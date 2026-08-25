#!/bin/bash
# Do the two read a font name the same way?
#
# `FcNameParse` is the syntax every fontconfig command line speaks --
# `"DejaVu Sans-12:bold:lang=en"` -- and until this harness existed nothing
# compared it. The other harnesses all *start* from a parsed pattern, so a
# term read wrongly here reaches them as a query neither side disagrees
# about, and they pass.
#
# What it caught: constants were not resolved at all. `:weight=bold` reached
# matching as the string "bold" where fontconfig has the number 200, and
# `:bold` on its own was dropped entirely.
#
# Compared before substitution, which is what `fc-pattern` prints without
# `-c`: this is about reading the name, not about what the rules then do
# with it.
#
# Run: bash scripts/parse_parity.sh
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
cargo build -q --release --example fc_match || exit 1
MATCH="$CARGO_TARGET_DIR/release/examples/fc_match"
command -v fc-pattern >/dev/null || { echo "fc-pattern not found"; exit 1; }

# Every object either side can produce from a name, so a value landing on the
# wrong property is caught as well as a value read wrongly.
OBJECTS="family familylang style weight width slant size pixelsize spacing \
         rgba hintstyle antialias hinting autohint verticallayout outline \
         scalable decorative embolden embeddedbitmap minspace lcdfilter \
         index foundry lang charset fontversion globaladvance"

NAMES=(
  # Families, commas and escapes.
  "DejaVu Sans"
  "DejaVu Sans,Liberation Sans"
  "Foo\-Bar"
  "Foo\,Bar"
  "Foo\:Bar"
  ""
  ","
  ":"
  # Sizes, which a bare `-` introduces.
  "DejaVu Sans-12"
  "DejaVu Sans-12,18"
  "DejaVu Sans-Bold"
  "-14"
  # Named constants standing alone, which name their own property.
  ":bold"
  ":italic"
  ":oblique"
  ":charcell"
  ":hintfull"
  ":antialias"
  ":scalable"
  ":lcdlegacy"
  ":normal"
  ":notaconstant"
  "DejaVu Sans:bold:italic"
  # The same names as values, where the property decides and a constant
  # belonging to another one is refused.
  ":weight=bold"
  ":weight=BOLD"
  ":weight=book"
  ":weight=medium"
  ":width=normal"
  ":width=condensed"
  ":width=ultraexpanded"
  ":width=bold"
  ":slant=roman"
  ":slant=italic"
  ":slant=bold"
  ":spacing=mono"
  ":spacing=charcell"
  ":rgba=none"
  ":rgba=vbgr"
  ":hintstyle=hintslight"
  ":lcdfilter=lcdlight"
  ":antialias=antialias"
  # Numbers, ranges and the fall-through when a value is neither.
  ":weight=200"
  ":weight=[50 100]"
  ":weight=[light bold]"
  ":weight=[light 200]"
  ":weight=notanumber"
  ":size=12"
  ":size=12.5"
  ":pixelsize=14"
  ":index=2"
  ":index=notanumber"
  ":fontversion=65536"
  # Booleans, which have their own spellings.
  ":outline=true"
  ":outline=False"
  ":outline=yes"
  ":outline=0"
  ":outline=dontcare"
  ":outline=nonsense"
  ":scalable=on"
  ":embolden=off"
  # Language and character sets.
  ":lang=en"
  ":lang=ja"
  ":lang=en|de|fr"
  ":charset=41 42 43"
  # `_` separates a property from its value wherever `=` would.
  ":weight_200"
  ":lang_ja"
  # Several values for one property, and several properties.
  ":family=Alpha,Beta"
  ":weight=100,200"
  "Alpha:weight=200:slant=100:width=75"
  "Alpha-10:weight=bold:lang=ja"
  # Properties this crate knows and fontconfig may not fill in.
  ":foundry=PfEd"
  ":familylang=ja"
)

TOTAL=0
DIFFS=0
for name in "${NAMES[@]}"; do
  bad=""
  for object in $OBJECTS; do
    case "$object" in
      # `fc-pattern`'s listing prints these two as a bitmap, so a canonical
      # spelling has to come from a format string instead.
      lang|charset)
        theirs=$(fc-pattern --format="%{$object}" -- "$name" 2>/dev/null </dev/null) ;;
      *)
        theirs=$(fc-pattern -- "$name" 2>/dev/null </dev/null \
                 | python3 scripts/lib/field.py theirs "$object") ;;
    esac
    ours=$("$MATCH" --no-substitute --dump-query -- "$name" 2>/dev/null </dev/null \
           | python3 scripts/lib/field.py ours "$object")
    TOTAL=$((TOTAL + 1))
    if [ "$theirs" != "$ours" ]; then
      bad="$bad
      $object: fc=[$theirs] ours=[$ours]"
      DIFFS=$((DIFFS + 1))
    fi
  done
  if [ -z "$bad" ]; then
    printf '  %-34s MATCH\n' "$name"
  else
    printf '  %-34s DIFF%s\n' "$name" "$bad"
    fail
  fi
done

echo
echo "parse parity: $((TOTAL - DIFFS))/$TOTAL fields identical over ${#NAMES[@]} name(s)"
exit $((FAILURES > 0))
