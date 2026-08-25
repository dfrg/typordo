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
# Run: bash scripts/scan_parity.sh
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
OURS="$CARGO_TARGET_DIR/release/examples/fc_query"

FILES=/tmp/scan-files.txt
fc-list --format='%{file}\n' | sort -u > $FILES
echo "files: $(wc -l < $FILES)"

FIELDS="${*:-file index fontwrapper fontformat outline color scalable fonthashint
        foundry order fontversion weight width slant spacing family style
        fullname postscriptname charset lang decorative symbol capability
        namedinstance variable properties}"

# fc-query prints one block per face, each property on its own line.
NAMES=/tmp/scan-names.py
cat > $NAMES <<'PY'
import re, sys
names = []
for line in sys.stdin:
    if line.startswith('Pattern has'):
        if names:
            print(','.join(names))
        names = []
        continue
    m = re.match(r'\t([a-z]+):', line)
    if m:
        names.append(m.group(1))
if names:
    print(','.join(names))
PY

# Fields where a difference is expected, and how many files should show it.
# A deliberate divergence that reads as a failure forever teaches you to
# ignore the harness, and a silenced one hides the day it changes. So the
# count is named: matching it reports KNOWN, anything else reports DIFF.
# See "Divergences we chose" in docs/gaps.md.
expected_diff() {
  case "$1" in
    capability) echo 3 ;;   # named instances of .ttc collections
    properties) echo 21 ;;  # the same three files, one line per face
    *) echo 0 ;;
  esac
}

total_ok=0; total_bad=0
for field in $FIELDS; do
  "$OURS" --format "$field" --batch < $FILES > /tmp/scan-ours.txt 2>/dev/null
  : > /tmp/scan-theirs.txt
  if [ "$field" = properties ]; then
    # Which properties a pattern *has*, not what one of them says. An element
    # that exists with an empty value prints exactly like an absent one, and
    # the two score differently: the comparison skips a property only one
    # side has, so an absent language says nothing while an empty one says
    # "answers nothing". A whole class of difference the other fields cannot
    # see, and one that hid a real bug until Adlam arrived in the corpus.
    while IFS= read -r f; do
      echo "@$f" >> /tmp/scan-theirs.txt
      fc-query "$f" </dev/null 2>/dev/null | python3 "$NAMES" >> /tmp/scan-theirs.txt
    done < $FILES
  else
    while IFS= read -r f; do
      echo "@$f" >> /tmp/scan-theirs.txt
      fc-query --format="%{${field}}\n" "$f" </dev/null >> /tmp/scan-theirs.txt 2>/dev/null
    done < $FILES
  fi

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
  want=$(expected_diff "$field")
  if [ "$bad" -eq 0 ]; then
    printf '  %-16s MATCH  %s\n' "$field" "$ok"
  elif [ "$bad" -eq "$want" ]; then
    printf '  %-16s KNOWN  %s ok, %s differing as expected\n' "$field" "$ok" "$bad"
  else
    printf '  %-16s DIFF   %s ok, %s differing (expected %s)\n' \
      "$field" "$ok" "$bad" "$want"
      fail
  fi
done
echo
echo "scan parity: $total_ok identical, $total_bad differing"

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
