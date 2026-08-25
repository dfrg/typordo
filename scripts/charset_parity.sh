#!/bin/bash
# Compare our charset decoding against fc-query, font file by font file.
# Run: bash scripts/charset_parity.sh
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
cargo build -q --release --example charset || exit 1

# A .ttc or variable font contributes several patterns, and fc-query prints
# one charset per face -- so the format string needs its own newline, or the
# faces run together into a single line.
FMT='%{charset}\n'

ok=0; bad=0
for f in $(fc-list --format='%{file}\n' | sort -u); do
  ours=$(cargo run -q --release --example charset -- "$f" 2>/dev/null)
  theirs=$(fc-query --format="$FMT" "$f" 2>/dev/null)
  if [ "$ours" = "$theirs" ]; then
    ok=$((ok+1))
  else
    bad=$((bad+1))
    if [ $bad -le 3 ]; then
      echo "MISMATCH: $f"
      echo "  ours   ${#ours} chars, $(echo "$ours" | wc -l) lines"
      echo "  theirs ${#theirs} chars, $(echo "$theirs" | wc -l) lines"
      diff <(echo "$ours" | tr ' ' '\n') <(echo "$theirs" | tr ' ' '\n') | head -8
    fi
  fi
done
echo "charset parity: $ok identical, $bad differing"
[ "$bad" -eq 0 ] || fail

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
