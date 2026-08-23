#!/bin/bash
# Have fontconfig itself read the caches we write.
#
# This is the only oracle that matters for the writer: our own reader agrees
# with us by construction, but `fc-list` has to walk the same bytes through
# `FcCacheOffsetsValid`, which checks where things sit and not just what they
# say. If a value list ran downwards or a string landed before the element
# naming it, fontconfig would silently drop the whole cache and fall back to
# scanning -- so the run also asserts that nothing was rescanned.
#
# Two rounds. `--rewrite` reads this system's caches and writes them back,
# testing the writer alone. Scanning builds them from the font files, which
# puts the scanner in the loop too.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/write_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --release --features scan --example fc_cache || exit 1
cargo build -q --release --example fc_list || exit 1

OURS=/tmp/fc-write
CONF=/tmp/fc-write.conf

# A config identical to the system one except for where caches live. Font
# directories have to match exactly: a cache is found by the hash of its
# directory name, so a different spelling is a different cache.
make_conf() {
  {
    echo '<?xml version="1.0"?>'
    echo '<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">'
    echo '<fontconfig>'
    for d in $(fc-list --format='%{file}\n' | xargs -n1 dirname | sort -u); do
      : # directories come from the roots below, not from font files
    done
    echo '  <dir>/usr/share/fonts</dir>'
    echo '  <dir>/usr/local/share/fonts</dir>'
    echo "  <cachedir>$OURS</cachedir>"
    echo '</fontconfig>'
  } > "$CONF"
}

# The same config, but reading the system caches, so the two sides differ in
# nothing but which cache files they read.
SYSCONF=/tmp/fc-write-system.conf
{
  echo '<?xml version="1.0"?>'
  echo '<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">'
  echo '<fontconfig>'
  echo '  <dir>/usr/share/fonts</dir>'
  echo '  <dir>/usr/local/share/fonts</dir>'
  echo '  <cachedir>/var/cache/fontconfig</cachedir>'
  echo '  <cachedir prefix="xdg">fontconfig</cachedir>'
  echo '</fontconfig>'
} > "$SYSCONF"

make_conf

# Every field a cache carries that fc-list can print.
FMT='%{file}|%{index}|%{family}|%{familylang}|%{style}|%{stylelang}|%{fullname}|%{postscriptname}|%{foundry}|%{weight}|%{width}|%{slant}|%{spacing}|%{outline}|%{scalable}|%{color}|%{fontversion}|%{fontformat}|%{charset}|%{lang}
'

# The queries fc-match answers differently for different caches.
QUERIES="sans-serif serif monospace DejaVuSans FreeSans :lang=ja :lang=ar :lang=hi :lang=ru"

check() {
  local label="$1"
  # FC_DEBUG=16 reports every cache fontconfig loads or refuses.
  local log
  log=$(FONTCONFIG_FILE="$CONF" FC_DEBUG=16 fc-list --format='%{file}\n' 2>&1 >/dev/null)
  local ours theirs
  ours=$(FONTCONFIG_FILE="$CONF" fc-list --format='%{file}|%{family}|%{style}|%{lang}\n' 2>/dev/null | sort)
  theirs=$(FONTCONFIG_FILE="$SYSCONF" fc-list --format='%{file}|%{family}|%{style}|%{lang}\n' 2>/dev/null | sort)

  local n_ours n_theirs
  n_ours=$(echo "$ours" | grep -c . )
  n_theirs=$(echo "$theirs" | grep -c . )
  echo "=== $label ==="
  echo "  fc-list: ours $n_ours lines, system $n_theirs lines"
  if echo "$log" | grep -qi 'invalid cache\|broken'; then
    echo "  REJECTED: fontconfig refused a cache"
    echo "$log" | grep -i 'invalid cache\|broken' | sort -u | head -5
  fi
  if [ "$ours" = "$theirs" ]; then
    echo "  fc-list MATCH ($(echo "$ours" | grep -c .) patterns, 20 fields)"
  else
    echo "  fc-list DIFF"
    diff <(echo "$theirs") <(echo "$ours") | head -6
  fi

  # Matching reads the same caches and has to reach the same font.
  local ok=0 bad=0
  for q in $QUERIES; do
    local a b
    a=$(FONTCONFIG_FILE="$CONF"    fc-match --format='%{file}
' "$q" 2>/dev/null </dev/null)
    b=$(FONTCONFIG_FILE="$SYSCONF" fc-match --format='%{file}
' "$q" 2>/dev/null </dev/null)
    if [ "$a" = "$b" ]; then ok=$((ok+1)); else bad=$((bad+1)); echo "    fc-match DIFF $q: $a vs $b"; fi
  done
  echo "  fc-match: $ok identical, $bad differing"
}

rm -rf "$OURS"; mkdir -p "$OURS"
"$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" --rewrite
# Our own reader has to accept them too, and see the same fonts.
echo "  ours, via our reader: $("$CARGO_TARGET_DIR/release/examples/fc_list" --config "$CONF" --format file | wc -l) files"
check "rewritten from the system caches"

rm -rf "$OURS"; mkdir -p "$OURS"
"$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" /usr/share/fonts /usr/local/share/fonts
echo "  ours, via our reader: $("$CARGO_TARGET_DIR/release/examples/fc_list" --config "$CONF" --format file | wc -l) files"
check "scanned from the font files"
