#!/bin/bash
# Compare our font scanning against fc-query, field by field.
#
# fc-query re-scans the font file rather than reading a cache, which makes it
# the right oracle here and the wrong one for everything else in this repo.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/scan_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example fc_query || exit 1

FILES=/tmp/scan-files.txt
fc-list --format='%{file}\n' | sort -u > $FILES
echo "files: $(wc -l < $FILES)"

total_ok=0; total_bad=0
for field in file index fontwrapper fontformat outline color scalable \
             fonthashint foundry order fontversion weight width slant \
             spacing family style fullname postscriptname; do
  ok=0; bad=0; shown=0
  while IFS= read -r f; do
    ours=$(cargo run -q --example fc_query -- --format "$field" "$f" 2>/dev/null </dev/null)
    theirs=$(fc-query --format="%{${field}}\n" "$f" 2>/dev/null </dev/null)
    if [ "$ours" = "$theirs" ]; then
      ok=$((ok+1))
    else
      bad=$((bad+1))
      if [ $shown -lt 2 ]; then
        printf '      %s\n        ours   %s\n        theirs %s\n' \
          "$(basename "$f")" "$(echo "$ours" | head -2 | tr '\n' '/')" "$(echo "$theirs" | head -2 | tr '\n' '/')"
        shown=$((shown+1))
      fi
    fi
  done < $FILES
  total_ok=$((total_ok+ok)); total_bad=$((total_bad+bad))
  if [ "$bad" -eq 0 ]; then printf '  %-16s MATCH  %s\n' "$field" "$ok"
  else printf '  %-16s DIFF   %s ok, %s differing\n' "$field" "$ok" "$bad"; fi
done
echo
echo "scan parity: $total_ok identical, $total_bad differing"
