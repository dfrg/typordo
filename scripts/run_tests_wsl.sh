#!/bin/bash
# Run the test suite under WSL, where the font files the scan tests need exist.
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo test "$@" 2>&1 | grep -E '^test |test result|panicked|assertion|left:|right:'
