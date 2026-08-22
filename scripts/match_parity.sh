#!/bin/bash
# Compare our matching against fc-match, over the real font set.
#
# The config deliberately has no <match> rules: this slice implements
# scoring, not the substitution pass that rewrites a query first. Handing
# both implementations the same rule-free config isolates one from the other.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/match_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example fc_match || exit 1

CONF=/tmp/fontconf-match.conf
cat > $CONF <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>/usr/share/fonts</dir>
  <cachedir prefix="xdg">fontconfig</cachedir>
  <cachedir>/usr/lib/fontconfig/cache</cachedir>
</fontconfig>
EOF

ok=0; bad=0; tied=0; failed=()
check() {
  local q="$1"
  local ours theirs
  ours=$(cargo run -q --example fc_match -- --config "$CONF" "$q" 2>/dev/null)
  theirs=$(FONTCONFIG_FILE="$CONF" fc-match --format='%{file}
' "$q" 2>/dev/null)
  if [ "$ours" = "$theirs" ]; then
    ok=$((ok+1)); return
  fi
  # Differing is not the same as wrong. Fontconfig breaks an exact tie by
  # taking whichever font its internal hash table happened to yield first,
  # which is not reproducible from outside. Score their pick with our own
  # scorer: if it ties ours, both answers are defensible.
  local sa sb
  sa=$(cargo run -q --example fc_match -- --config "$CONF" --score-of "$ours" "$q" 2>/dev/null)
  sb=$(cargo run -q --example fc_match -- --config "$CONF" --score-of "$theirs" "$q" 2>/dev/null)
  if [ -n "$sa" ] && [ "$sa" = "$sb" ]; then
    tied=$((tied+1)); return
  fi
  bad=$((bad+1)); failed+=("$q")
  if [ ${#failed[@]} -le 12 ]; then
    printf 'DIFF  %-40s
        ours   %s
        theirs %s
'       "$q" "${ours:-<none>}" "${theirs:-<none>}"
  fi
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

echo "=== lang, which this slice does not score ==="
lang_ok=0; lang_bad=0
for q in ":lang=en" ":lang=ar" ":lang=ja" ":lang=fa" "DejaVu Sans:lang=ru"; do
  before=$bad; check "$q"
  if [ $bad -gt $before ]; then lang_bad=$((lang_bad+1)); else lang_ok=$((lang_ok+1)); fi
done
echo "  lang queries: $lang_ok identical, $lang_bad differing (expected to differ)"

echo
echo "match parity: $ok identical, $tied tie-broken differently, $bad genuinely differing"
if [ $bad -gt 0 ]; then
  echo "failing queries:"
  printf '  %s\n' "${failed[@]}" | head -20
fi
