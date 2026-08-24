#!/bin/bash
# Verify cache file *names* against real fontconfig.
#
# A cache is found by hashing the directory path, and three things change what
# gets hashed: a `salt` attribute, a `<remap-dir>` naming a different path,
# and a `.uuid` file in the font directory. Getting any of them wrong fails
# silently -- a cache that cannot be found is not an error, it just means a
# rescan -- so the names are compared directly.
#
# The oracle is the file fontconfig writes: run it against a config with one
# font directory and an empty cache directory, and whatever lands there is
# the name it chose.
#
# Run: bash scripts/name_parity.sh
set -uo pipefail

# A harness is a check, not a report. Anything that differs has to make the
# script fail, or a caller running it -- CI most of all -- is told everything
# passed while it is looking at differences.
FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -q --release --example dirs || exit 1
OURS="$CARGO_TARGET_DIR/release/examples/dirs"

root=$(mktemp -d)
fonts="$root/fonts"
conf="$root/fonts.conf"
cache="$root/cache"
mkdir -p "$fonts/sub"

# $1 label, $2 the directory under test, $3.. the config body.
check() {
  local label="$1" dir="$2"; shift 2
  {
    echo '<?xml version="1.0"?>'
    echo '<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">'
    echo '<fontconfig>'
    printf '%s\n' "$@"
    echo "  <cachedir>$cache</cachedir>"
    echo '</fontconfig>'
  } > "$conf"

  rm -rf "$cache"; mkdir -p "$cache"
  # fc-cache descends, so several caches land here. Pick the one that says it
  # describes the directory under test rather than whichever sorts first.
  FONTCONFIG_FILE="$conf" fc-cache -f "$dir" >/dev/null 2>&1
  local theirs
  theirs=$(python3 - "$cache" "$dir" <<'PYX'
import os, struct, sys
cache, want = sys.argv[1], sys.argv[2]
for name in sorted(os.listdir(cache)):
    if not name.endswith('cache-9'):
        continue
    d = open(os.path.join(cache, name), 'rb').read()
    # The header holds an offset to the directory name, relative to itself.
    at = struct.unpack_from('<q', d, 16)[0]
    end = d.index(bytes([0]), at)
    if d[at:end].decode('utf-8', 'replace') == want:
        print(name)
        break
PYX
)
  local ours
  ours=$("$OURS" --config "$conf" --cache-name "$dir")
  if [ "$ours" = "$theirs" ]; then
    echo "MATCH   $label: $ours"
  else
    echo "DIFF    $label: ours=$ours fontconfig=${theirs:-<none written>}"
    fail
  fi
}

check "plain" "$fonts" "  <dir>$fonts</dir>"

check "salted" "$fonts" "  <dir salt=\"pepper\">$fonts</dir>"

# A salt applies to everything beneath the directory that carries it, not
# only to the directory itself.
check "salted subdir" "$fonts/sub" "  <dir salt=\"pepper\">$fonts</dir>"

# A different salt has to give a different name, or the attribute does nothing.
check "other salt" "$fonts" "  <dir salt=\"other\">$fonts</dir>"

# <remap-dir> hashes the path it is told to pretend to be.
check "remapped" "$fonts" "  <remap-dir as-path=\"/usr/share/fonts\">$fonts</remap-dir>"

# And the remapping applies to the prefix, so a subdirectory keeps its tail.
check "remapped subdir" "$fonts/sub" "  <remap-dir as-path=\"/usr/share/fonts\">$fonts</remap-dir>"

check "remapped and salted" "$fonts" \
  "  <remap-dir as-path=\"/usr/share/fonts\" salt=\"pepper\">$fonts</remap-dir>"

# Fontconfig takes the first font directory that contains the path, not the
# longest, so a plain <dir> listed first shadows a <remap-dir> beneath it.
check "shadowed by an earlier dir" "$fonts/sub" \
  "  <dir>$fonts</dir>" \
  "  <remap-dir as-path=\"/usr/share/fonts\" salt=\"pepper\">$fonts/sub</remap-dir>"

# A <remap-dir> with no as-path says nothing and is dropped.
check "remap without as-path" "$fonts" \
  "  <dir>$fonts</dir>" \
  "  <remap-dir>$fonts</remap-dir>"

echo "=== .uuid ==="
{
  echo '<?xml version="1.0"?>'
  echo '<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">'
  echo '<fontconfig>'
  echo "  <dir>$fonts</dir>"
  echo "  <cachedir>$cache</cachedir>"
  echo '</fontconfig>'
} > "$conf"
uuid=4b5a9f2e-8c31-4d7a-9e0f-1a2b3c4d5e6f
printf '%s' "$uuid" > "$fonts/.uuid"
rm -rf "$cache"; mkdir -p "$cache"

# The hashed name is what gets written; the uuid one is only ever a fallback.
hashed=$("$OURS" --config "$conf" --cache-name "$fonts")
touch "$cache/$uuid-le64.cache-9"
found=$("$OURS" --config "$conf" --cache-path "$fonts")
case "$found" in
  *"$uuid"*) echo "MATCH   uuid fallback used when nothing else is there" ;;
  *) echo "DIFF    uuid fallback not used: ${found:-<nothing>}" ; fail ;;
esac

# ...and the hashed name still wins when both exist.
touch "$cache/$hashed"
found=$("$OURS" --config "$conf" --cache-path "$fonts")
case "$found" in
  *"$hashed") echo "MATCH   the hashed name wins over the uuid one" ;;
  *) echo "DIFF    uuid should be a fallback only: $found" ; fail ;;
esac

rm -rf "$root"

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
