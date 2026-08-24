#!/bin/bash
# Compare our langset decoding against fc-list.
#
# fc-list reads the caches, which is what we read. fc-query re-scans the font
# file instead, so it can legitimately disagree with a cache written earlier
# -- it is the wrong oracle for a cache reader, and using it made the CJK
# .ttc files look broken when they were not.
#
# Run: bash scripts/lang_parity.sh
set -uo pipefail

# A harness is a check, not a report. Anything that differs has to make the
# script fail, or a caller running it -- CI most of all -- is told everything
# passed while it is looking at differences.
FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo build -q --release --example langs || exit 1

cargo run -q --release --example langs 2>/dev/null | sort > /tmp/lang_ours.txt
fc-list --format='%{file}\t%{lang}\n' | sort > /tmp/lang_theirs.txt

echo "ours   $(wc -l < /tmp/lang_ours.txt) lines"
echo "theirs $(wc -l < /tmp/lang_theirs.txt) lines"
if diff -q /tmp/lang_ours.txt /tmp/lang_theirs.txt > /dev/null; then
  echo "langset parity: identical"
else
  echo "langset parity: DIFFERING"
  fail
  diff /tmp/lang_ours.txt /tmp/lang_theirs.txt | head -12
  echo "  differing lines: $(diff /tmp/lang_ours.txt /tmp/lang_theirs.txt | grep -c '^<')"
fi

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
