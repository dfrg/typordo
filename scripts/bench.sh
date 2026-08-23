#!/bin/bash
# Time this crate against libfontconfig, doing the same work.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/bench.sh
#
# What the numbers mean, because a benchmark that is not read carefully is
# worse than none:
#
#  * `config` and `load` are measured **per process**, once each, because
#    fontconfig keeps every cache it has read in a process-wide table.
#    Looping them in one process would time its memoisation against our real
#    work. Those rows are wall-clock time for the whole invocation, so they
#    include process start and dynamic linking -- which is the honest number
#    for a command-line tool, and why `noop` is shown beside them: it is the
#    same measurement with the work removed.
#
#  * `list`, `match` and `sort` loop inside one process after loading once,
#    which is what a running program does.
#
#  * Both sides validate every cache on load. Fontconfig runs
#    FcCacheOffsetsValid, which walks every pattern and value, exactly as our
#    Cache::validate does -- so this is not a handicap either side is carrying
#    alone.
#
#  * Numbers are only comparable within one font set. The corpus here has
#    changed size more than once; anything quoted from an older run was
#    measured against a different amount of work.
#
#  * Our default build *reads* cache files; fontconfig *maps* anything over
#    1KiB. That is a real difference in what `load` costs, so the run reports
#    our number both ways.
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="/usr/bin:/bin:$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"

REPEATS=${REPEATS:-9}   # odd, so the median is a real sample
BIN="$CARGO_TARGET_DIR/release/examples/bench"
BIN_MMAP="$CARGO_TARGET_DIR/mmap/release/examples/bench"
FC=/tmp/bench_fc

echo "building"
cargo build -q --release --example bench || exit 1
CARGO_TARGET_DIR="$CARGO_TARGET_DIR/mmap" cargo build -q --release --features mmap --example bench || exit 1
gcc -O2 -o "$FC" scripts/bench_fc.c $(pkg-config --cflags --libs fontconfig) || exit 1

# The median of REPEATS runs, in nanoseconds. `$4` picks the clock: `inner`
# is what the binary timed around the work, `outer` is wall time for the
# whole invocation, which includes starting the process and linking it.
median() {
  local out
  if [ "${4:-inner}" = outer ]; then
    out=$(for _ in $(seq "$REPEATS"); do
            local a b
            a=$(date +%s%N); "$1" "$2" "$3" > /dev/null; b=$(date +%s%N)
            echo $((b - a))
          done | sort -n)
  else
    out=$(for _ in $(seq "$REPEATS"); do "$1" "$2" "$3" | cut -d' ' -f3; done | sort -n)
  fi
  echo "$out" | sed -n "$(( (REPEATS + 1) / 2 ))p"
}

# One row: operation, iterations, and both sides.
row() {
  local op="$1" iters="$2" clock="${3:-inner}"
  local ours theirs mm
  ours=$(median "$BIN" "$op" "$iters" "$clock")
  mm=$(median "$BIN_MMAP" "$op" "$iters" "$clock")
  theirs=$(median "$FC" "$op" "$iters" "$clock")
  python3 - "$op" "$iters" "$ours" "$mm" "$theirs" <<'PY'
import sys
op, iters, ours, mm, theirs = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
def per(total):
    return total / iters
def show(ns):
    if ns >= 1e6:
        return f"{ns/1e6:8.2f} ms"
    if ns >= 1e3:
        return f"{ns/1e3:8.2f} us"
    return f"{ns:8.0f} ns"
ratio = theirs / ours if ours else 0.0
print(f"{op:<8} {iters:>6}  ours {show(per(ours))}  +mmap {show(per(mm))}  "
      f"fontconfig {show(per(theirs))}   {ratio:5.2f}x")
# Every number printed is per operation: the totals are divided by the
# iteration count before they get here. Dividing again is a mistake that has
# already been made once.
PY
}

echo
echo "warming the page cache"
"$BIN" load 1 > /dev/null; "$FC" load 1 > /dev/null

echo
echo "per process, wall clock, including process start and linking"
row noop 1 outer
row config 1 outer
row load 1 outer

echo
echo "in process, after loading once"
row list 20
row prepare 500
row match 500
row sort 200

echo
echo "what a fallback picker asks: eight characters and a language, no family"
row charmatch 300
row charsort 200

echo
echo "the same, with the family the caller was already using"
row hintmatch 300
row hintsort 200

echo
echo "the last column is fontconfig's time divided by ours: above 1.00 is us"
echo "being faster. Checksums, to show both sides did the same work:"
for op in list match sort charmatch charsort hintmatch hintsort; do
  printf '  %-6s ours %-14s fontconfig %s\n' "$op" \
    "$("$BIN" "$op" 5 | cut -d' ' -f4)" "$("$FC" "$op" 5 | cut -d' ' -f4)"
done
