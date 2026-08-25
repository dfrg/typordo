#!/bin/bash
# Compare `<test>` operators against fontconfig, one operator at a time.
#
# The other harnesses drive whole queries through real fonts, and a font set
# reaches only the comparisons its fonts happen to provoke: nothing in a
# normal corpus carries a charset test, a langset test, or a range on both
# sides, so those went unchecked while three of them were wrong. Here each
# case is a `<match>` whose single `<test>` is the thing under test, over a
# pattern chosen to exercise it, and the question is only whether the test
# fired.
#
# Run: bash scripts/compare_parity.sh
set -uo pipefail

FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
# An absolute path, or cargo builds inside the repository; see prepare_parity.
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2; exit 1 ;;
esac
cargo build -q --release --example fc_match || exit 1

command -v fc-pattern >/dev/null || { echo "fc-pattern not found"; exit 1; }

D=$(mktemp -d) || exit 1
trap 'rm -rf "$D"' EXIT

# The edit fires only if the test passes, so the marker's presence is the
# answer. `foundry` is used because it survives config substitution untouched
# and no default rule assigns it.
run_case() { # $1 = <test> element, $2 = pattern
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?>
<fontconfig>
<match target="pattern">
  $1
  <edit name="foundry" mode="assign"><string>FIRED</string></edit>
</match>
</fontconfig>
XML
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c "$2" 2>/dev/null | grep -c FIRED)
  ours=$(cargo run -q --release --example fc_match -- \
           --config "$D/f.conf" --dump-query "$2" 2>/dev/null | grep -c FIRED)
  [ "$theirs" -gt 0 ] && theirs=pass || theirs=fail
  [ "$ours" -gt 0 ] && ours=pass || ours=fail
  if [ "$ours" = "$theirs" ]; then
    printf '  %-42s %-4s MATCH\n' "$3" "$theirs"
  else
    printf '  %-42s ours=%-4s theirs=%-4s DIFF\n' "$3" "$ours" "$theirs"
    fail
  fi
}

# An `<alias>` may carry `<test>` elements of its own, which make it
# conditional. They are a different code path from `<match>`, so they get
# their own runner, keyed on whether the preferred family was prepended.
run_alias() { # $1 = tests, $2 = pattern
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?>
<fontconfig>
<alias>
  $1
  <family>serif</family>
  <prefer><family>MarkerFamily</family></prefer>
</alias>
</fontconfig>
XML
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c "$2" 2>/dev/null | grep -c MarkerFamily)
  ours=$(cargo run -q --release --example fc_match --            --config "$D/f.conf" --dump-query "$2" 2>/dev/null | grep -c MarkerFamily)
  [ "$theirs" -gt 0 ] && theirs=pass || theirs=fail
  [ "$ours" -gt 0 ] && ours=pass || ours=fail
  if [ "$ours" = "$theirs" ]; then
    printf '  %-42s %-4s MATCH
' "$3" "$theirs"
  else
    printf '  %-42s ours=%-4s theirs=%-4s DIFF
' "$3" "$ours" "$theirs"
    fail
  fi
}

t() { echo "<test name=\"$1\" compare=\"$2\">$3</test>"; }
str() { echo "<string>$1</string>"; }
num() { echo "<double>$1</double>"; }
rng() { echo "<range><double>$1</double><double>$2</double></range>"; }

echo "=== a string against a language set (FcLangSetPromote)"
run_case "$(t lang contains "$(str en)")"    ':lang=en'    "{en} contains en"
run_case "$(t lang contains "$(str de)")"    ':lang=en'    "{en} contains de"
run_case "$(t lang contains "$(str en-gb)")" ':lang=en'    "{en} contains en-gb"
run_case "$(t lang contains "$(str en)")"    ':lang=en-gb' "{en-gb} contains en"
run_case "$(t lang eq "$(str en)")"          ':lang=en'    "{en} eq en"
run_case "$(t lang not_eq "$(str de)")"      ':lang=en'    "{en} not_eq de"

echo "=== a number against a range (FcRangePromote)"
# The direction matters and is easy to get backwards: `contains` asks whether
# the left falls inside the right, so a range does not contain a number it
# spans -- the number, promoted to a point, has to contain the range.
run_case "$(t size contains "$(num 12)")"     ':size=[10 20]' "[10,20] contains 12"
run_case "$(t size contains "$(rng 10 20)")"  ':size=12'      "12 contains [10,20]"
run_case "$(t size contains "$(rng 10 20)")"  ':size=30'      "30 contains [10,20]"
run_case "$(t size eq "$(num 12)")"           ':size=[12 12]' "[12,12] eq 12"
run_case "$(t size eq "$(num 12)")"           ':size=[10 20]' "[10,20] eq 12"

echo "=== two ranges (FcRangeCompare)"
run_case "$(t size less "$(rng 6 9)")"     ':size=[1 5]'  "[1,5] less [6,9]"
run_case "$(t size less "$(rng 6 9)")"     ':size=[1 6]'  "[1,6] less [6,9]"
run_case "$(t size less_eq "$(rng 6 9)")"  ':size=[1 6]'  "[1,6] less_eq [6,9]"
run_case "$(t size more "$(rng 1 6)")"     ':size=[7 9]'  "[7,9] more [1,6]"
run_case "$(t size more_eq "$(rng 1 6)")"  ':size=[6 9]'  "[6,9] more_eq [1,6]"
run_case "$(t size not_eq "$(rng 1 6)")"   ':size=[1 7]'  "[1,7] not_eq [1,6]"

echo "=== numbers"
run_case "$(t size less "$(num 6)")"     ':size=12' "12 less 6"
run_case "$(t size less "$(num 6)")"     ':size=5'  "5 less 6"
run_case "$(t size more_eq "$(num 6)")"  ':size=6'  "6 more_eq 6"

echo "=== booleans"
run_case "$(t scalable less_eq '<bool>true</bool>')"  ':scalable=True' "true less_eq true"
run_case "$(t scalable less_eq '<bool>false</bool>')" ':scalable=True' "true less_eq false"
run_case "$(t scalable less '<bool>false</bool>')"    ':scalable=True' "true less false"
run_case "$(t scalable eq '<bool>true</bool>')"       ':scalable=True' "true eq true"

echo "=== types that cannot be brought together"
run_case "$(t family not_eq '<int>1</int>')"       ':family=Foo' "'Foo' not_eq 1"
run_case "$(t family eq '<int>1</int>')"           ':family=Foo' "'Foo' eq 1"
run_case "$(t family not_contains '<int>1</int>')" ':family=Foo' "'Foo' not_contains 1"
run_case "$(t family less '<int>1</int>')"         ':family=Foo' "'Foo' less 1"

echo "=== a conditional <alias> (FcParseAlias keeps its tests)"
run_alias "$(t lang contains "$(str ja)")" 'serif:lang=ja' "ja alias, ja query"
run_alias "$(t lang contains "$(str ja)")" 'serif:lang=de' "ja alias, de query"
run_alias "$(t lang contains "$(str ja)")" 'sans-serif:lang=ja' "ja alias, other family"
run_alias "" 'serif:lang=de' "unconditional alias"
run_alias "$(t lang contains "$(str ja)")$(t weight more_eq '<int>200</int>')"   'serif:lang=ja:weight=200' "two tests, both pass"
run_alias "$(t lang contains "$(str ja)")$(t weight more_eq '<int>200</int>')"   'serif:lang=ja:weight=80' "two tests, one fails"

echo
if [ "$FAILURES" -gt 0 ]; then
  echo "compare parity: $FAILURES case(s) DIFF"
else
  echo "compare parity: every case MATCH"
fi
exit $((FAILURES > 0))
