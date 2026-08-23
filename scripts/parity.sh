#!/bin/bash
# Compare our reader against the system fc-list, driven by the real config.
# Run: bash scripts/parity.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"

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
