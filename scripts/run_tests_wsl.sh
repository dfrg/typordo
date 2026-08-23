#!/bin/bash
# The test suite as Linux sees it. Some behaviour is genuinely
# platform-specific -- how a directory reports having changed, most of all --
# so passing on Windows is not passing.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/run_tests_wsl.sh
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo test -q --features scan 2>&1 | grep -E 'test result|FAILED|panicked|^error' 
cargo clippy -q --all-targets --features scan 2>&1 | grep -E '^(warning|error)' | head -5
echo "clippy done"
