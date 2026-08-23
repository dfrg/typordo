#!/bin/bash
# Compare our langset decoding against fc-list.
#
# fc-list reads the caches, which is what we read. fc-query re-scans the font
# file instead, so it can legitimately disagree with a cache written earlier
# -- it is the wrong oracle for a cache reader, and using it made the CJK
# .ttc files look broken when they were not.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/lang_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --release --example langs || exit 1

cargo run -q --release --example langs 2>/dev/null | sort > /tmp/lang_ours.txt
fc-list --format='%{file}\t%{lang}\n' | sort > /tmp/lang_theirs.txt

echo "ours   $(wc -l < /tmp/lang_ours.txt) lines"
echo "theirs $(wc -l < /tmp/lang_theirs.txt) lines"
if diff -q /tmp/lang_ours.txt /tmp/lang_theirs.txt > /dev/null; then
  echo "langset parity: identical"
else
  echo "langset parity: DIFFERING"
  diff /tmp/lang_ours.txt /tmp/lang_theirs.txt | head -12
  echo "  differing lines: $(diff /tmp/lang_ours.txt /tmp/lang_theirs.txt | grep -c '^<')"
fi
