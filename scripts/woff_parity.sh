#!/bin/bash
# Compare WOFF and WOFF2 scanning against fc-query, field by field.
#
# A web font is an SFNT with its tables compressed. Fontconfig reads one
# through FreeType, which unpacks it and queries the result; this crate does
# the same with `wuff`, behind the `woff` feature. Without that feature a
# `.woff` is "not a font file" here and a font there, which is the difference
# this checks.
#
# The corpus is whatever the machine happens to have -- rustdoc ships several
# WOFF2 files with every Rust toolchain, which is the most reliable source of
# them on a developer machine. A run that finds none says so rather than
# passing quietly.
#
# Run: bash scripts/woff_parity.sh
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
cargo build -q --release --features woff --example fc_query || exit 1
QUERY="$CARGO_TARGET_DIR/release/examples/fc_query"
command -v fc-query >/dev/null || { echo "fc-query not found"; exit 1; }

# Every property fc-query reports that a web font can carry.
FIELDS="family style fullname weight width slant spacing foundry fontformat
        outline scalable charset lang capability fontversion index
        postscriptname decorative symbol variable namedinstance color
        fonthashint order"

FILES=$(
  { find "${RUSTUP_HOME:-$HOME/.rustup}" -name '*.woff2' 2>/dev/null
    find /usr/share /usr/local/share "$HOME" -name '*.woff' -o -name '*.woff2' 2>/dev/null
  } | sort -u | head -40
)

if [ -z "$FILES" ]; then
  echo "woff parity: no WOFF or WOFF2 files on this machine, nothing compared"
  exit 0
fi

files=0; total=0; bad=0
for font in $FILES; do
  files=$((files + 1))
  differing=0
  for field in $FIELDS; do
    theirs=$(fc-query --format="%{$field}\n" "$font" 2>&1 | head -1)
    ours=$("$QUERY" --format "$field" "$font" 2>&1 | head -1)
    total=$((total + 1))
    if [ "$theirs" != "$ours" ]; then
      bad=$((bad + 1))
      differing=$((differing + 1))
      # Two per file is enough to identify the problem without a wall of text.
      if [ "$differing" -le 2 ]; then
        printf '  DIFF %s %s\n    ours:   %.60s\n    theirs: %.60s\n' \
          "$(basename "$font")" "$field" "$ours" "$theirs"
      fi
    fi
  done
  [ "$differing" -gt 0 ] && fail
done

echo "woff parity: $((total - bad))/$total fields identical over $files file(s)"
exit $((FAILURES > 0))
