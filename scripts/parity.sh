#!/bin/bash
# Compare our reader against the system fc-list, driven by the real config.
# Run: bash scripts/parity.sh
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

echo "=== building ==="
cargo build -q --release --example fc_list || exit 1
run() { cargo run -q --release --example fc_list -- "$@"; }

echo "=== what the config found ==="
run --dirs

compare() {
  local label="$1" ours="$2" theirs="$3"
  if diff -q <(sort -u "$ours") <(sort -u "$theirs") > /dev/null; then
    echo "MATCH ($label): $(sort -u "$theirs" | wc -l) lines identical"
  else
    echo "DIFF ($label): ours=$(sort -u "$ours" | wc -l) fc-list=$(sort -u "$theirs" | wc -l)"
    fail
    diff <(sort -u "$ours") <(sort -u "$theirs") | head -20
  fi
}

echo
echo "=== parity ==="
run --format file --stats > /tmp/rust_files.txt
fc-list --format='%{file}\n' > /tmp/fc_files.txt
compare "file" /tmp/rust_files.txt /tmp/fc_files.txt

run > /tmp/rust_full.txt
fc-list --format='%{file}: %{family}:style=%{style}\n' > /tmp/fc_full.txt
compare "file+family+style" /tmp/rust_full.txt /tmp/fc_full.txt

run --format family > /tmp/rust_fam.txt
fc-list --format='%{family}\n' | sed 's/,.*//' > /tmp/fc_fam.txt
compare "first family" /tmp/rust_fam.txt /tmp/fc_fam.txt

echo
echo "=== cache basenames agree with the files on disk ==="
missing=0
for dir in $(fc-list --format='%{file}\n' | sed 's|/[^/]*$||' | sort -u); do
  base=$(printf '%s' "$dir" | md5sum | cut -d' ' -f1)-le64.cache-9
  if [ ! -f "$HOME/.cache/fontconfig/$base" ] && [ ! -f "/usr/lib/fontconfig/cache/$base" ]; then
    echo "  no cache for $dir"; missing=$((missing+1))
  fi
done
echo "  $missing directories without a cache"

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
