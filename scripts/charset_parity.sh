#!/bin/bash
# Compare our charset decoding against fc-query, font file by font file.
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/charset_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example charset || exit 1

# A .ttc or variable font contributes several patterns, and fc-query prints
# one charset per face -- so the format string needs its own newline, or the
# faces run together into a single line.
FMT='%{charset}\n'

ok=0; bad=0
for f in $(fc-list --format='%{file}\n' | sort -u); do
  ours=$(cargo run -q --example charset -- "$f" 2>/dev/null)
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
