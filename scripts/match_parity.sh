#!/bin/bash
# Compare our matching against fc-match, over the real font set.
#
# The config deliberately has no <match> rules: this slice implements
# scoring, not the substitution pass that rewrites a query first. Handing
# both implementations the same rule-free config isolates one from the other.
#
# Run: bash scripts/match_parity.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -q --release --example fc_match || exit 1

# REAL=1 runs against the system's own /etc/fonts, rules and all. Without it
# the config is rule-free, which isolates scoring from substitution.
if [ "${REAL:-0}" = "1" ]; then
  CONF=/etc/fonts/fonts.conf
else
  CONF=/tmp/typordo-match.conf
  cat > $CONF <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>/usr/share/fonts</dir>
  <cachedir prefix="xdg">fontconfig</cachedir>
  <cachedir>/usr/lib/fontconfig/cache</cachedir>
</fontconfig>
EOF
fi
echo "config: $CONF"

QUERIES=/tmp/fc-queries.txt
: > $QUERIES
check() { printf '%s
' "$1" >> $QUERIES; }

run_all() {
  local n; n=$(wc -l < $QUERIES)
  echo
  echo "running $n queries"
  # Ours in one process: loading every cache costs far more than matching.
  cargo run -q --release --example fc_match -- --config "$CONF" --batch < $QUERIES > /tmp/fc-ours.txt
  # fc-match has no batch mode, so this is one process per query.
  : > /tmp/fc-theirs.txt
  while IFS= read -r q; do
    FONTCONFIG_FILE="$CONF" fc-match --format='%{file}
' "$q" </dev/null >> /tmp/fc-theirs.txt 2>/dev/null || echo >> /tmp/fc-theirs.txt
  done < $QUERIES

  # Two passes. The first is pure text and runs no subprocess, so nothing can
  # steal the loop's stdin; the second re-checks only the mismatches.
  #
  # The field separator is , not a tab: `read` strips leading IFS
  # *whitespace*, so with a tab the empty-query test's blank first field
  # vanished and every field shifted left, inventing a mismatch that the same
  # three files do not contain.
  ok=0; bad=0; tied=0; failed=()
  : > /tmp/fc-mismatch.txt
  paste -d$'' $QUERIES /tmp/fc-ours.txt /tmp/fc-theirs.txt > /tmp/fc-joined.txt
  while IFS=$'' read -r q ours theirs; do
    if [ "$ours" = "$theirs" ]; then
      ok=$((ok+1))
    else
      printf '%s%s%s
' "$q" "$ours" "$theirs" >> /tmp/fc-mismatch.txt
    fi
  done < /tmp/fc-joined.txt

  # Differing is not the same as wrong. Fontconfig breaks an exact tie by
  # taking whichever font its hash table yielded first, which is not
  # reproducible from outside. Score their pick with our own scorer: if it
  # ties ours, both answers are defensible.
  local total; total=$(wc -l < /tmp/fc-mismatch.txt)
  for i in $(seq 1 "$total"); do
    local line q ours theirs sa sb
    line=$(sed -n "${i}p" /tmp/fc-mismatch.txt)
    q=$(printf '%s' "$line" | cut -d$'' -f1)
    ours=$(printf '%s' "$line" | cut -d$'' -f2)
    theirs=$(printf '%s' "$line" | cut -d$'' -f3)
    sa=$(cargo run -q --release --example fc_match -- --config "$CONF" --score-of "$ours" "$q" 2>/dev/null </dev/null)
    sb=$(cargo run -q --release --example fc_match -- --config "$CONF" --score-of "$theirs" "$q" 2>/dev/null </dev/null)
    if [ -n "$sa" ] && [ "$sa" = "$sb" ]; then
      tied=$((tied+1)); continue
    fi
    bad=$((bad+1)); failed+=("$q")
    if [ ${#failed[@]} -le 12 ]; then
      printf 'DIFF  %s
        ours   %s
        theirs %s
'         "$q" "${ours:-<none>}" "${theirs:-<none>}"
    fi
  done
}

echo "=== every installed family, by name ==="
while IFS= read -r fam; do check "$fam"; done < <(
  FONTCONFIG_FILE="$CONF" fc-list --format='%{family}\n' | sed 's/,.*//' | sort -u)

echo "=== styles and axes ==="
for q in \
  "DejaVu Sans:weight=200" "DejaVu Sans:weight=80" "DejaVu Sans:slant=100" \
  "DejaVu Sans:width=75" "DejaVu Sans:weight=200:slant=100" \
  "DejaVu Sans Mono:spacing=100" "Noto Sans:weight=40" \
  "Cantarell:weight=210" "Liberation Sans:slant=110" \
  "DejaVu Sans:pixelsize=24" "DejaVu Sans:size=8" ; do
  check "$q"
done

echo "=== case and blanks in the family name ==="
for q in "dejavu sans" "DEJAVUSANS" "  DejaVu   Sans  " "dejavusans" "LiberationSans"; do
  check "$q"
done

echo "=== a family nothing has, and multi-family fallback ==="
for q in "No Such Family" "No Such Family,DejaVu Sans" "DejaVu Sans,No Such Family" \
         "No Such Family,Also Missing,Liberation Sans" ; do
  check "$q"
done

echo "=== every family crossed with weight, slant and width ==="
while IFS= read -r fam; do
  for prop in "weight=40" "weight=200" "weight=210" "slant=100" "slant=110"               "width=75" "width=125" "spacing=100" "weight=200:slant=100"; do
    check "$fam:$prop"
  done
done < <(FONTCONFIG_FILE="$CONF" fc-list --format='%{family}
' | sed 's/,.*//' | sort -u)

echo "=== generic aliases (no rules to expand them under this config) ==="
for q in "serif" "sans-serif" "monospace" "system-ui" "cursive" "fantasy" "emoji"; do
  check "$q"
done

echo "=== sizes ==="
for q in "DejaVu Sans:size=6" "DejaVu Sans:size=72" "DejaVu Sans:pixelsize=8"          "DejaVu Sans:pixelsize=100" "Noto Sans:size=10.5"; do
  check "$q"
done

echo "=== postscriptname, foundry, file, fontformat ==="
for q in "DejaVuSans" "DejaVuSans-Bold" "NotoSansArabic-Regular"          ":foundry=unknown" ":fontformat=TrueType" ":fontformat=CFF"; do
  check "$q"
done

echo "=== booleans that participate in scoring ==="
for q in ":scalable=true" ":scalable=false" ":outline=true" ":color=true"          ":variable=true" ":variable=false" ":decorative=true" ":symbol=true"; do
  check "$q"
done

echo "=== the empty query ==="
check ""

echo "=== language requests ==="
for q in ":lang=en" ":lang=ar" ":lang=ja" ":lang=ko" ":lang=fa" ":lang=ru"          ":lang=zh-cn" ":lang=zh-tw" ":lang=he" ":lang=hi" ":lang=th" ":lang=el"          ":lang=vi" ":lang=tr" ":lang=ur" ":lang=bn" ":lang=ta" ":lang=km"          ":lang=en-us" ":lang=en-gb" ":lang=pt-br" ":lang=zh-hk" ":lang=und"          ":lang=xx" ":lang=nonsense"          "DejaVu Sans:lang=ru" "Noto Sans:lang=ar" "serif:lang=ja"          "sans-serif:lang=ko" "monospace:lang=en" ; do
  check "$q"
done

echo "=== language crossed with weight and slant ==="
for l in en ar ja fa ru zh-cn; do
  for p in "weight=200" "slant=100" "weight=40:slant=110"; do
    check ":lang=$l:$p"
  done
done

run_all

echo
echo "match parity: $ok identical, $tied tie-broken differently, $bad genuinely differing"
if [ $bad -gt 0 ]; then
  echo "failing queries:"
  printf '  %s\n' "${failed[@]}" | head -20
fi
