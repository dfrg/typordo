#!/bin/bash
# Compare the prepared pattern against fc-match, field by field.
#
# Matching only has to pick the right font; render_prepare has to reconstruct
# the whole answer -- the font's values narrowed to the one that matched, the
# query's own values carried across, and the font-target rules applied. Only
# comparing %{file} would never exercise any of that.
#
# Run: bash scripts/prepare_parity.sh
set -uo pipefail

# A harness is a check, not a report. Anything that differs has to make the
# script fail, or a caller running it -- CI most of all -- is told everything
# passed while it is looking at differences.
FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
# An absolute path, or cargo builds inside the repository. That is not
# hypothetical: a shell that mangled `$HOME` once handed these scripts
# `C:Userscbrok/fct`, which has no leading slash, and cargo dutifully created
# it here -- where `git add -A` then committed it. Twice.
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *)
    echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2
    exit 1
    ;;
esac
cargo build -q --release --example fc_match || exit 1

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
  cargo run -q --release --example fc_match -- --config "$CONF" --format "$field" --batch \
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
# Localized names, which the field sweep above cannot reach: it runs under an
# English locale, so the name a query would promote is always the one already
# first. A font with names in two languages and a query naming one of them is
# the only shape that shows whether promotion happens at all.
localized=0; localized_bad=0
MULTILINGUAL=$(fc-list --format='%{familylang}|%{family}
' 2>/dev/null | python3 -c "
import sys
for line in sys.stdin:
    langs, _, families = line.partition('|')
    # More than one distinct language, so there is something to choose between.
    if len(set(langs.split(','))) > 1:
        print(families.split(',')[0].strip())
        break
")
if [ -n "$MULTILINGUAL" ]; then
  echo "=== localized names ($MULTILINGUAL) ==="
  for lang in ja en zh-cn de; do
    for field in family familylang style stylelang fullname fullnamelang; do
      q="$MULTILINGUAL:familylang=$lang"
      theirs=$(FONTCONFIG_FILE="$CONF" fc-match --format="%{$field}" "$q" </dev/null 2>/dev/null)
      ours=$(cargo run -q --release --example fc_match --                --config "$CONF" --format "$field" "$q" 2>/dev/null)
      localized=$((localized + 1))
      if [ "$theirs" != "$ours" ]; then
        localized_bad=$((localized_bad + 1))
        printf '  DIFF familylang=%-6s %-14s
    ours:   %.60s
    theirs: %.60s
'           "$lang" "$field" "$ours" "$theirs"
      fi
    done
  done
  # And through the locale, which is how a desktop actually asks.
  for locale in ja_JP.UTF-8 en_US.UTF-8; do
    theirs=$(LC_ALL=$locale FONTCONFIG_FILE="$CONF" fc-match --format='%{family}' "$MULTILINGUAL" </dev/null 2>/dev/null)
    ours=$(LC_ALL=$locale cargo run -q --release --example fc_match --              --config "$CONF" --format family "$MULTILINGUAL" 2>/dev/null)
    localized=$((localized + 1))
    if [ "$theirs" != "$ours" ]; then
      localized_bad=$((localized_bad + 1))
      printf '  DIFF LC_ALL=%-14s family
    ours:   %.60s
    theirs: %.60s
'         "$locale" "$ours" "$theirs"
    fi
  done
  printf '  %-16s %s   %s ok, %s differing
' "localized"     "$([ "$localized_bad" -eq 0 ] && echo MATCH || echo DIFF)"     "$((localized - localized_bad))" "$localized_bad"
  [ "$localized_bad" -eq 0 ] || fail
else
  echo "=== localized names: no font here carries names in two languages ==="
fi

echo
echo "prepare parity: $total_ok identical, $total_bad differing"
[ "$total_bad" -eq 0 ] || fail

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
