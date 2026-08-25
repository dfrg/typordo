#!/bin/bash
# Compare bare-CFF scanning against fc-query, field by field.
#
# A `CFF ` table on its own is a font to FreeType and so to fontconfig, and a
# thinner one than the OpenType font it usually sits inside: no `name` table,
# no `OS/2`, no `cmap`. Everything comes from the CFF's own top dictionary and
# its charset, which makes almost every field a separate derivation and worth
# comparing separately.
#
# The corpus is built rather than found: bare CFF files are rare on disk, so
# this extracts the `CFF ` table out of every OpenType font the machine has.
# That is also the honest corpus, since it is exactly the bytes a caller would
# have if they pulled one out themselves.
#
# Run: bash scripts/cff_parity.sh
set -uo pipefail

FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2; exit 1 ;;
esac
cargo build -q --release --example fc_query || exit 1
QUERY="$CARGO_TARGET_DIR/release/examples/fc_query"
command -v fc-query >/dev/null || { echo "fc-query not found"; exit 1; }

WORK=$(mktemp -d) || exit 1
trap 'rm -rf "$WORK"' EXIT

python3 - "$WORK" <<'PY'
import os, struct, subprocess, sys
work = sys.argv[1]
files = sorted(set(subprocess.run(["fc-list", "--format=%{file}\n"],
                                  capture_output=True, text=True).stdout.split()))
extracted = 0
for path in files:
    if not path.lower().endswith((".otf", ".ttf", ".ttc", ".otc")):
        continue
    try:
        data = open(path, "rb").read()
    except OSError:
        continue
    # One face only: a collection's directory is laid out differently and the
    # point here is the CFF table, not the wrapper.
    if len(data) < 12 or data[:4] not in (b"OTTO", b"\x00\x01\x00\x00", b"true"):
        continue
    count = struct.unpack(">H", data[4:6])[0]
    for i in range(count):
        entry = 12 + i * 16
        if entry + 16 > len(data):
            break
        tag, _checksum, offset, length = struct.unpack(">4sIII", data[entry:entry + 16])
        if tag == b"CFF " and offset + length <= len(data) and length > 4:
            name = os.path.basename(path).rsplit(".", 1)[0]
            open(f"{work}/{name}.cff", "wb").write(data[offset:offset + length])
            extracted += 1
            break
print(f"extracted {extracted} bare CFF tables")
PY

FIELDS="family style fullname postscriptname weight width slant spacing
        foundry fontformat fontversion outline scalable index decorative
        symbol variable namedinstance color fonthashint order capability
        charset lang"

shopt -s nullglob
files=0; total=0; bad=0
for font in "$WORK"/*.cff; do
  files=$((files + 1))
  differing=0
  for field in $FIELDS; do
    theirs=$(fc-query --format="%{$field}\n" "$font" 2>&1 | head -1)
    ours=$("$QUERY" --format "$field" "$font" 2>&1 | head -1)
    total=$((total + 1))
    if [ "$theirs" != "$ours" ]; then
      bad=$((bad + 1))
      differing=$((differing + 1))
      if [ "$differing" -le 3 ]; then
        printf '  DIFF %s %s\n    ours:   %.70s\n    theirs: %.70s\n' \
          "$(basename "$font")" "$field" "$ours" "$theirs"
      fi
    fi
  done
  [ "$differing" -gt 0 ] && fail
done

if [ "$files" -eq 0 ]; then
  echo "cff parity: no CFF-flavoured fonts on this machine, nothing compared"
  exit 0
fi
echo "cff parity: $((total - bad))/$total fields identical over $files file(s)"
exit $((FAILURES > 0))
