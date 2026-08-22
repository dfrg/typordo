#!/bin/bash
# Compare our cache reader against the system fc-list.
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"   # never build into the Windows target/

echo "=== building ==="
cargo build -q --example fc_list || exit 1

CACHE="$HOME/.cache/fontconfig"
echo "=== our reader ==="
cargo run -q --example fc_list -- "$CACHE" --format file --stats > /tmp/rust_files.txt
echo "=== fc-list ==="
fc-list --format='%{file}\n' | sort -u > /tmp/fc_files.txt

echo "=== FILE parity ==="
if diff -q <(sort -u /tmp/rust_files.txt) /tmp/fc_files.txt > /dev/null; then
  echo "MATCH: $(wc -l < /tmp/fc_files.txt) files identical"
else
  echo "DIFF:"
  diff <(sort -u /tmp/rust_files.txt) /tmp/fc_files.txt | head -20
fi

echo
echo "=== full-format parity (file: families:style=styles) ==="
cargo run -q --example fc_list -- "$CACHE" > /tmp/rust_full.txt
fc-list --format='%{file}: %{family}:style=%{style}\n' | sort -u > /tmp/fc_full.txt
if diff -q <(sort -u /tmp/rust_full.txt) /tmp/fc_full.txt > /dev/null; then
  echo "MATCH: $(wc -l < /tmp/fc_full.txt) lines identical"
else
  echo "ours=$(wc -l < /tmp/rust_full.txt) fc-list=$(wc -l < /tmp/fc_full.txt)"
  diff <(sort -u /tmp/rust_full.txt) /tmp/fc_full.txt | head -20
fi
