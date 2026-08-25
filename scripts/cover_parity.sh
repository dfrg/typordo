#!/bin/bash
# Compare scanned charsets and language sets against fc-query.
# Run: bash scripts/cover_parity.sh
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
cargo build -q --release --example fc_query || exit 1
fc-list --format='%{file}\n' | sort -u > /tmp/scan-files.txt

# Which fields to compare, both by default.
#
# `lang` is worth naming separately because it is the one comparison that is
# version-specific: the language list is generated from a particular
# fontconfig release, and an older fontconfig cannot report the languages
# added since. See docs/gaps.md.
FIELDS="${*:-charset lang}"

for field in $FIELDS; do
  ok=0; bad=0; shown=0
  while IFS= read -r f; do
    ours=$(cargo run -q --release --example fc_query -- --format "$field" "$f" 2>/dev/null </dev/null)
    theirs=$(fc-query --format="%{${field}}\n" "$f" 2>/dev/null </dev/null)
    if [ "$ours" = "$theirs" ]; then ok=$((ok+1)); else
      bad=$((bad+1))
      if [ $shown -lt 3 ]; then
        echo "  DIFF $(basename "$f")"
        fail
        diff <(echo "$ours" | head -1 | tr ' |' '\n\n') <(echo "$theirs" | head -1 | tr ' |' '\n\n') | head -8
        shown=$((shown+1))
      fi
    fi
  done < /tmp/scan-files.txt
  printf '  %-10s %s identical, %s differing\n' "$field" "$ok" "$bad"
done

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
