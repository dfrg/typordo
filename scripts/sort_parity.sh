#!/bin/bash
# Compare our font sorting against fc-match -s and -a.
#
# A sort is not a ranking. Fontconfig demotes any font that answers no
# language the query still needs, then -s keeps only fonts that draw a
# character the ones before them could not. -a skips that trimming. Both are
# checked, because they exercise different halves.
#
# Run: bash scripts/sort_parity.sh
set -uo pipefail

# A harness is a check, not a report. Anything that differs has to make the
# script fail, or a caller running it -- CI most of all -- is told everything
# passed while it is looking at differences.
FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -q --release --example fc_match || exit 1

CONF=${CONF:-/etc/fonts/fonts.conf}
echo "config: $CONF"

QUERIES=(
  "sans-serif" "serif" "monospace"
  "DejaVu Sans" "Liberation Serif" "Noto Sans" "Cantarell" "Terminus"
  "DejaVu Sans:weight=200" "Noto Sans:slant=100"
  ":lang=en" ":lang=ja" ":lang=ar" ":lang=ru" ":lang=zh-cn" ":lang=ko"
  ":lang=he" ":lang=th" ":lang=hi" ":lang=ta" ":lang=km" ":lang=am"
  "sans-serif:lang=ja" "serif:lang=ar" "monospace:lang=ru"
  "No Such Family" ""
  "DejaVu Sans:lang=en" "Noto Sans:lang=ar:weight=200"
)

for mode in sort all; do
  flag="--$mode"; fcflag="-s"
  [ "$mode" = all ] && fcflag="-a"
  ok=0; bad=0
  echo "=== $mode (fc-match $fcflag) ==="
  for q in "${QUERIES[@]}"; do
    ours=$(cargo run -q --release --example fc_match -- --config "$CONF" $flag --format file "$q" 2>/dev/null </dev/null)
    theirs=$(FONTCONFIG_FILE="$CONF" fc-match $fcflag --format='%{file}\n' "$q" 2>/dev/null </dev/null)
    if [ "$ours" = "$theirs" ]; then
      ok=$((ok+1))
    else
      bad=$((bad+1))
      if [ $bad -le 3 ]; then
        echo "  DIFF q=${q:-<empty>}"
        echo "    ours   $(echo "$ours"   | wc -l) entries"
        echo "    theirs $(echo "$theirs" | wc -l) entries"
        diff <(echo "$ours") <(echo "$theirs") | head -8
      fi
    fi
  done
  echo "  $mode parity: $ok identical, $bad differing"
  [ "$bad" -eq 0 ] || fail
done

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
