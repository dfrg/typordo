#!/bin/bash
# Compare scanning of symbol-encoded fonts against fc-query.
#
# A symbol font addresses its glyphs through a Windows (3, 0) cmap rather than
# a Unicode one, and fontconfig reads it differently in three ways: the
# coverage comes from that table, the U+F000 range is copied down to U+0000,
# and the language set is empty however much Latin-1 the copy appears to
# cover.
#
# Separate from the other harnesses because the corpus decides whether this
# runs at all. A Linux font set typically has no symbol font -- there are
# none among the 2385 here -- so this hunts for one and says plainly when it
# finds nothing, rather than passing in silence.
#
# Run: bash scripts/symbol_parity.sh
set -uo pipefail

FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -q --release --example fc_query || exit 1
OURS="$CARGO_TARGET_DIR/release/examples/fc_query"

# Anything installed, plus the conventional homes of the fonts that made this
# encoding worth having.
{
  fc-list --format='%{file}\n' 2>/dev/null
  ls /mnt/c/Windows/Fonts/*.ttf /c/Windows/Fonts/*.ttf 2>/dev/null
  ls "$HOME"/.local/share/fonts/*.ttf 2>/dev/null
} | sort -u > /tmp/symbol-candidates.txt

found=0
while IFS= read -r f; do
  [ -r "$f" ] || continue
  [ "$(fc-query --format='%{symbol}' "$f" 2>/dev/null </dev/null)" = "True" ] || continue
  found=$((found + 1))
  for field in charset lang symbol; do
    ours=$("$OURS" --format "$field" "$f" 2>/dev/null </dev/null)
    theirs=$(fc-query --format="%{${field}}" "$f" 2>/dev/null </dev/null)
    if [ "$ours" = "$theirs" ]; then
      printf '  MATCH  %-10s %s\n' "$field" "$(basename "$f")"
    else
      printf '  DIFF   %-10s %s\n' "$field" "$(basename "$f")"
      printf '    ours   %s\n    theirs %s\n' "${ours:0:60}" "${theirs:0:60}"
      fail
    fi
  done
done < /tmp/symbol-candidates.txt

echo
if [ "$found" -eq 0 ]; then
  echo "symbol parity: no symbol font found, so nothing was compared"
else
  echo "symbol parity: $found font(s) compared"
fi

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
