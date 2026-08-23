#!/bin/bash
# The whole test suite, across the feature matrix.
#
# Some behaviour is genuinely platform-specific -- how a directory reports
# having changed, whether a `.uuid` file is consulted -- and `statfs` only
# compiles on Unix at all, so the matrix is worth running rather than
# assuming.
#
# Nothing here reaches for awk or bc, so it runs on a minimal PATH.
#
# Run: bash scripts/run_tests.sh
cd "$(dirname "$0")/.." || exit 1
export PATH="/usr/bin:/bin:$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"

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

# Every generated table still matches the generator that claims to produce
# it. This compares without writing, so a drifting generator is reported
# rather than allowed to overwrite the file it has drifted from.
#
# gen_name_langs needs FreeType's ttnameid.h for the numeric constants, which
# is not vendored -- it is skipped where that header is absent rather than
# counted as a failure.
bad=0
for g in tools/gen_*.py; do
  case "$g" in
    *gen_name_langs.py)
      [ -r /usr/include/freetype2/freetype/ttnameid.h ] || continue ;;
  esac
  python3 "$g" --check >/dev/null 2>&1 || { bad=$((bad + 1)); echo "  DRIFTED: $g"; }
done
printf '%-42s %s drifted\n' "generated tables" "$bad"
