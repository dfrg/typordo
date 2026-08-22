#!/bin/bash
# Compare the prepared pattern against fc-match, field by field.
#
# Matching only has to pick the right font; render_prepare has to reconstruct
# the whole answer -- the font's values narrowed to the one that matched, the
# query's own values carried across, and the font-target rules applied. Only
# comparing %{file} would never exercise any of that.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/prepare_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example fc_match || exit 1

CONF=${CONF:-/etc/fonts/fonts.conf}
echo "config: $CONF"

QUERIES=/tmp/prep-queries.txt
{
  fc-list --format='%{family}\n' | sed 's/,.*//' | sort -u
  printf '%s\n' \
    "sans-serif" "serif" "monospace" \
    "DejaVu Sans:weight=200" "DejaVu Sans:slant=100" \
    "Cantarell:weight=210" "Cantarell:weight=123" \
    "Noto Sans:lang=ar" ":lang=ja" ":lang=ko" ":lang=zh-cn" \
    "DejaVu Sans:size=8" "DejaVu Sans:pixelsize=24" \
    "Vazirmatn:weight=150" "Source Code Pro:weight=300" \
    "No Such Family" ""
} > $QUERIES
echo "queries: $(wc -l < $QUERIES)"

total_ok=0; total_bad=0
for field in file family style weight slant width spacing fontformat \
             foundry postscriptname fontversion index outline scalable \
             pixelsize size dpi hintstyle antialias fontvariations; do
  cargo run -q --example fc_match -- --config "$CONF" --format "$field" --batch \
    < $QUERIES > /tmp/prep-ours.txt 2>/dev/null
  : > /tmp/prep-theirs.txt
  while IFS= read -r q; do
    FONTCONFIG_FILE="$CONF" fc-match --format="%{${field}}\n" "$q" </dev/null \
      >> /tmp/prep-theirs.txt 2>/dev/null || echo >> /tmp/prep-theirs.txt
  done < $QUERIES

  same=$(paste -d$'\001' /tmp/prep-ours.txt /tmp/prep-theirs.txt \
    | python3 -c "
import sys
ok=bad=0
for line in sys.stdin:
    a,_,b = line.rstrip('\n').partition('\001')
    if a==b: ok+=1
    else: bad+=1
print(ok,bad)")
  ok=${same% *}; bad=${same#* }
  total_ok=$((total_ok+ok)); total_bad=$((total_bad+bad))
  if [ "$bad" -eq 0 ]; then
    printf '  %-16s MATCH  %s\n' "$field" "$ok"
  else
    printf '  %-16s DIFF   %s ok, %s differing\n' "$field" "$ok" "$bad"
    paste -d$'\001' $QUERIES /tmp/prep-ours.txt /tmp/prep-theirs.txt | python3 -c "
import sys
shown=0
for line in sys.stdin:
    parts=line.rstrip('\n').split('\001')
    if len(parts)==3 and parts[1]!=parts[2] and shown<3:
        print('      q=%r\n        ours   %r\n        theirs %r' % tuple(parts))
        shown+=1"
  fi
done
echo
echo "prepare parity: $total_ok identical, $total_bad differing"
