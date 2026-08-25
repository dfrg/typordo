#!/bin/bash
# Do the two agree on how firmly a matched font holds each property?
#
# Matching does not only pick a font. `FcFontSetMatchInternal` rebuilds the
# winner before preparing it, giving each object one binding for all its
# values -- strong when that object's strong distance came in under 1000,
# weak otherwise -- and leaving objects with no matcher alone. Nothing in the
# comparison of *values* can see that, which is why it went unnoticed until
# the second audit; this harness compares the bindings themselves.
#
# Run: bash scripts/bind_parity.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2; exit 1 ;;
esac
cargo build -q --release --example fc_match || exit 1
MATCH="$CARGO_TARGET_DIR/release/examples/fc_match"
command -v fc-match >/dev/null || { echo "fc-match not found"; exit 1; }

DIFFS=0
CASES=0

# Queries chosen to move the family binding both ways and to leave some
# objects unmentioned: an exact family name matches strongly, a generic one
# reaches its font through an alias and so matches weakly, and neither
# mentions `foundry` or `fontversion` at all.
QUERIES=(
  "DejaVu Sans"
  "DejaVu Serif"
  "DejaVu Sans Mono"
  "sans-serif"
  "serif"
  "monospace"
  "DejaVu Sans:weight=200"
  "DejaVu Sans:slant=100"
  "DejaVu Sans:lang=ja"
  "DejaVu Sans:size=12"
  "sans-serif:weight=200"
  "sans-serif:lang=ru"
  "Nonexistent Family Name"
  "Nonexistent:weight=200"
  "DejaVu Sans,Nonexistent"
  "Nonexistent,DejaVu Sans"
  ":lang=ja"
  ":spacing=100"
)

for q in "${QUERIES[@]}"; do
  theirs=$(fc-match -v "$q" 2>/dev/null | python3 scripts/lib/bindings.py theirs | sort)
  ours=$("$MATCH" --dump-match "$q" 2>/dev/null | python3 scripts/lib/bindings.py ours | sort)
  CASES=$((CASES + 1))
  # Compared on the objects both sides report. A property one of them does
  # not produce at all is a scanner difference, which the scanner harnesses
  # are the ones to report; here it would only be noise.
  common=$(comm -12 <(echo "$theirs" | cut -d= -f1 | sort -u) \
                    <(echo "$ours" | cut -d= -f1 | sort -u))
  t=$(echo "$theirs" | grep -F -f <(echo "$common") | sort)
  o=$(echo "$ours" | grep -F -f <(echo "$common") | sort)
  if [ "$t" = "$o" ]; then
    printf '  %-34s %s\n' "$q" "MATCH"
  else
    printf '  %-34s %s\n' "$q" "DIFF"
    diff <(echo "$t") <(echo "$o") | head -8
    DIFFS=$((DIFFS + 1))
  fi
done

echo
echo "bindings parity: $((CASES - DIFFS))/$CASES queries agree"
exit $((DIFFS > 0))
