#!/bin/bash
# Compare our font scanning against fc-query, field by field.
#
# fc-query re-scans the font file rather than reading a cache, which makes it
# the right oracle here and the wrong one for everything else in this repo.
#
# Both sides run once per field over the whole file list rather than once per
# file: scanning is fast, starting a process is not, and this is ~14000
# comparisons.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/scan_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --release --example fc_query || exit 1
OURS="$CARGO_TARGET_DIR/release/examples/fc_query"

FILES=/tmp/scan-files.txt
fc-list --format='%{file}\n' | sort -u > $FILES
echo "files: $(wc -l < $FILES)"

FIELDS="${*:-file index fontwrapper fontformat outline color scalable fonthashint
        foundry order fontversion weight width slant spacing family style
        fullname postscriptname charset lang}"

total_ok=0; total_bad=0
for field in $FIELDS; do
  "$OURS" --format "$field" --batch < $FILES > /tmp/scan-ours.txt 2>/dev/null
  : > /tmp/scan-theirs.txt
  while IFS= read -r f; do
    echo "@$f" >> /tmp/scan-theirs.txt
    fc-query --format="%{${field}}\n" "$f" </dev/null >> /tmp/scan-theirs.txt 2>/dev/null
  done < $FILES

  read -r ok bad <<< "$(python3 - "$field" <<'PY'
import sys
def blocks(path):
    out, cur, name = {}, [], None
    for line in open(path, encoding='utf-8', errors='replace'):
        line = line.rstrip('\n')
        if line.startswith('@'):
            if name is not None: out[name] = cur
            name, cur = line[1:], []
        else:
            cur.append(line)
    if name is not None: out[name] = cur
    return out
a, b = blocks('/tmp/scan-ours.txt'), blocks('/tmp/scan-theirs.txt')
ok = bad = 0
shown = 0
for name in b:
    if a.get(name) == b[name]:
        ok += 1
    else:
        bad += 1
        if shown < 2:
            import os
            print("      %s" % os.path.basename(name), file=sys.stderr)
            print("        ours   %s" % ('/'.join(a.get(name, []))[:70]), file=sys.stderr)
            print("        theirs %s" % ('/'.join(b[name])[:70]), file=sys.stderr)
            shown += 1
print(ok, bad)
PY
)"
  total_ok=$((total_ok+ok)); total_bad=$((total_bad+bad))
  if [ "$bad" -eq 0 ]; then printf '  %-16s MATCH  %s\n' "$field" "$ok"
  else printf '  %-16s DIFF   %s ok, %s differing\n' "$field" "$ok" "$bad"; fi
done
echo
echo "scan parity: $total_ok identical, $total_bad differing"
