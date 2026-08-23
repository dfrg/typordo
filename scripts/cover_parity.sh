#!/bin/bash
# Compare scanned charsets and language sets against fc-query.
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/cover_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example fc_query || exit 1
fc-list --format='%{file}\n' | sort -u > /tmp/scan-files.txt

for field in charset lang; do
  ok=0; bad=0; shown=0
  while IFS= read -r f; do
    ours=$(cargo run -q --example fc_query -- --format "$field" "$f" 2>/dev/null </dev/null)
    theirs=$(fc-query --format="%{${field}}\n" "$f" 2>/dev/null </dev/null)
    if [ "$ours" = "$theirs" ]; then ok=$((ok+1)); else
      bad=$((bad+1))
      if [ $shown -lt 3 ]; then
        echo "  DIFF $(basename "$f")"
        diff <(echo "$ours" | head -1 | tr ' |' '\n\n') <(echo "$theirs" | head -1 | tr ' |' '\n\n') | head -8
        shown=$((shown+1))
      fi
    fi
  done < /tmp/scan-files.txt
  printf '  %-10s %s identical, %s differing\n' "$field" "$ok" "$bad"
done
