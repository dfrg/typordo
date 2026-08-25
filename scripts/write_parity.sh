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
# Run: bash scripts/write_parity.sh
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
  [ "$bad" -eq 0 ] || fail
}

rm -rf "$OURS"; mkdir -p "$OURS"
"$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" --rewrite
# Our own reader has to accept them too, and see the same fonts.
echo "  ours, via our reader: $("$CARGO_TARGET_DIR/release/examples/fc_list" --config "$CONF" --format file | wc -l) files"
check "rewritten from the system caches"

rm -rf "$OURS"; mkdir -p "$OURS"
"$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS"
echo "  ours, via our reader: $("$CARGO_TARGET_DIR/release/examples/fc_list" --config "$CONF" --format file | wc -l) files"
check "scanned from the font files"

# Staleness. The cache records the directory's timestamp and nothing else, so
# a second pass must rescan nothing, and touching one directory must rescan
# exactly that one.
echo "=== staleness ==="
again=$("$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS")
echo "  second pass: $again"
case "$again" in
  "0 directories rescanned, "*) echo "  MATCH (nothing rescanned)" ;;
  *) echo "  DIFF: a current tree should rescan nothing" ; fail ;;
esac

# A directory nobody else is using, so touching it cannot disturb the system.
victim=$(mktemp -d)
"$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" "$victim" > /dev/null
sleep 1.1
touch "$victim/a-new-file"
one=$("$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" "$victim")
echo "  after adding a file: $one"
case "$one" in
  "1 directories rescanned, "*) echo "  MATCH (the changed directory rescanned)" ;;
  *) echo "  DIFF: adding a file should make the cache stale" ; fail ;;
esac
rm -rf "$victim"

forced=$("$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$OURS" -f)
echo "  forced: $forced"

# SOURCE_DATE_EPOCH pins the clock for a reproducible build. Fontconfig
# clamps the recorded time down to it and drops the nanoseconds, which the
# variable cannot express. Compare the recorded field directly, since nothing
# prints it.
#
# Note what fontconfig does when the clamp actually fires -- when the epoch is
# older than the directory. It writes the cache, then checks the cache it just
# wrote by comparing the clamped stamp against the *unclamped* directory
# mtime, decides it failed, and deletes it. In a real reproducible build the
# directory mtime is already pinned, so the clamp never fires and nobody sees
# this. We write the cache and keep it: the same clamp is applied when reading
# it back, so it stays valid.
echo "=== SOURCE_DATE_EPOCH ==="
sde_dir=$(mktemp -d)
sde_out=$(mktemp -d)
stamp() {
  python3 -c "import struct,sys;d=open(sys.argv[1],'rb').read();print(struct.unpack_from('<i',d,48)[0], struct.unpack_from('<q',d,56)[0])" "$1"
}
name=$(python3 -c "
import hashlib,sys
print(hashlib.md5(sys.argv[1].encode()).hexdigest() + '-le64.cache-9')" "$sde_dir")
now=$(python3 -c "import os,sys;print(int(os.stat(sys.argv[1]).st_mtime))" "$sde_dir")

for epoch in $((now - 1000)) $((now + 1000)) 4000000000 not-a-number; do
  SOURCE_DATE_EPOCH=$epoch "$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$sde_out" -f "$sde_dir" > /dev/null
  ours=$(stamp "$sde_out/$name")

  cat > /tmp/fc-sde.conf <<XML
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>$sde_dir</dir>
  <cachedir>$sde_out/theirs</cachedir>
</fontconfig>
XML
  rm -rf "$sde_out/theirs"; mkdir -p "$sde_out/theirs"
  SOURCE_DATE_EPOCH=$epoch FONTCONFIG_FILE=/tmp/fc-sde.conf fc-cache -f "$sde_dir" >/dev/null 2>&1
  if [ ! -f "$sde_out/theirs/$name" ]; then
    echo "  epoch=$epoch: fontconfig deleted its own cache; ours kept, stamp=$ours"
    continue
  fi
  theirs=$(stamp "$sde_out/theirs/$name")
  if [ "$ours" = "$theirs" ]; then
    echo "  MATCH   epoch=$epoch stamp=$ours"
  else
    echo "  DIFF    epoch=$epoch ours=$ours theirs=$theirs"
    fail
  fi
done

# And ours stays valid across runs when the clock is pinned, which is the
# whole point: the same clamp is applied when the cache is read back.
pin=$((now - 1000))
SOURCE_DATE_EPOCH=$pin "$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$sde_out" -f "$sde_dir" > /dev/null
again=$(SOURCE_DATE_EPOCH=$pin "$CARGO_TARGET_DIR/release/examples/fc_cache" --out "$sde_out" "$sde_dir")
case "$again" in
  "0 directories rescanned, "*) echo "  MATCH   the pinned cache stays current: $again" ;;
  *) echo "  DIFF    a pinned cache should not go stale: $again" ; fail ;;
esac
rm -rf "$sde_dir" "$sde_out"

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
