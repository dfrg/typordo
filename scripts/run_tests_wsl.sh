#!/bin/bash
# The test suite as Linux sees it, across the feature matrix.
#
# Some behaviour is genuinely platform-specific -- how a directory reports
# having changed, whether a `.uuid` file is consulted -- so passing on Windows
# is not passing. And `statfs` only compiles on Unix at all.
#
# Nothing here reaches for awk or bc: invoked as `wsl bash <script>` the PATH
# is not the login one, and half the usual tools are missing.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/run_tests_wsl.sh
cd /mnt/c/Work/play/fontconf
export PATH="/usr/bin:/bin:$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"

total() {
  local sum=0 n
  while read -r n; do sum=$((sum + n)); done < <(
    echo "$1" | grep -oE '^test result: ok\. [0-9]+' | grep -oE '[0-9]+$'
  )
  echo "$sum"
}

for f in "" "--no-default-features" "--features mmap" "--features statfs" \
         "--features full-fontconfig-compat" "--all-features"; do
  out=$(cargo test -q $f 2>&1)
  bad=$(echo "$out" | grep -cE 'FAILED|^error')
  printf '%-42s %4s tests, %s problems\n' "cargo test $f" "$(total "$out")" "$bad"
done

for f in "" "--features full-fontconfig-compat"; do
  bad=$(cargo clippy -q --all-targets $f 2>&1 | grep -cE '^(warning|error)')
  printf '%-42s %s clippy issues\n' "clippy $f" "$bad"
done
