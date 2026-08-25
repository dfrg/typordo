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
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c -d "$2" 2>/dev/null | grep -c FIRED)
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
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c -d "$2" 2>/dev/null | grep -c MarkerFamily)
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

echo "=== the third state of a boolean (FcDontCare)"
# `FcNameBool` spells it `dontcare`, `d`, `x`, `2` or `or`, and the ordering
# operators exist to ask about it: `less` is "they differ and the right side
# is DontCare", which is the only reading under which `less` on a flag means
# anything at all.
b() { echo "<bool>$1</bool>"; }
run_case "$(t scalable eq "$(b dontcare)")"       ':scalable=dontcare' "dontcare eq dontcare"
run_case "$(t scalable eq "$(b true)")"           ':scalable=dontcare' "dontcare eq true"
run_case "$(t scalable not_eq "$(b true)")"       ':scalable=dontcare' "dontcare not_eq true"
run_case "$(t scalable contains "$(b true)")"     ':scalable=dontcare' "dontcare contains true"
run_case "$(t scalable contains "$(b true)")"     ':scalable=false'    "false contains true"
run_case "$(t scalable not_contains "$(b true)")" ':scalable=dontcare' "dontcare not_contains true"
run_case "$(t scalable less "$(b dontcare)")"     ':scalable=true'     "true less dontcare"
run_case "$(t scalable less "$(b dontcare)")"     ':scalable=dontcare' "dontcare less dontcare"
run_case "$(t scalable less_eq "$(b dontcare)")"  ':scalable=true'     "true less_eq dontcare"
run_case "$(t scalable more "$(b true)")"         ':scalable=dontcare' "dontcare more true"
run_case "$(t scalable more "$(b true)")"         ':scalable=false'    "false more true"
run_case "$(t scalable more_eq "$(b true)")"      ':scalable=dontcare' "dontcare more_eq true"
# The spellings, every one fontconfig accepts.
for spell in dontcare d x 2 or DontCare; do
  run_case "$(t scalable eq "$(b "$spell")")" ':scalable=dontcare' "spelling <bool>$spell</bool>"
done

echo "=== types that cannot be brought together"
run_case "$(t family not_eq '<int>1</int>')"       ':family=Foo' "'Foo' not_eq 1"
run_case "$(t family eq '<int>1</int>')"           ':family=Foo' "'Foo' eq 1"
run_case "$(t family not_contains '<int>1</int>')" ':family=Foo' "'Foo' not_contains 1"
run_case "$(t family less '<int>1</int>')"         ':family=Foo' "'Foo' less 1"

echo "=== <const> in a rule (FcNameGetConstant, case-insensitive)"
run_case '<test name="weight" compare="eq"><const>bold</const></test>' ':weight=200' "const bold vs 200"
run_case '<test name="weight" compare="eq"><const>Bold</const></test>' ':weight=200' "const Bold vs 200"
run_case '<test name="weight" compare="eq"><const>bold</const></test>' ':weight=80' "const bold vs 80"
run_case '<test name="weight" compare="eq"><const>nosuchconst</const></test>' ':weight=200' "const unknown"

# Reduce either side's notation to "<number> <TYPE>", so the comparison is
# about the value and its type rather than about how each prints them.
normalise() {
  python3 - "$1" "$2" <<'PY'
import re, sys
side, text = sys.argv[1], sys.argv[2].strip()
def trim(n):
    # 1.0 and 1 are the same number; only the type says which it is.
    return n[:-2] if n.endswith(".0") else n
if side == "theirs":
    text = re.sub(r"\((?:w|s)\)$", "", text)
    m = re.match(r"^(.*)\(i\)$", text)
    if m: print(f"{trim(m.group(1))} INT"); raise SystemExit
    m = re.match(r"^(.*)\(f\)$", text)
    if m: print(f"{trim(m.group(1))} DOUBLE"); raise SystemExit
    m = re.match(r"^\[(.*)\]$", text)
    if m:
        print("[%s] MATRIX" % " ".join(trim(p) for p in m.group(1).replace(";", " ; ").split()))
        raise SystemExit
else:
    m = re.match(r"^Int\((-?\d+)\)$", text)
    if m: print(f"{m.group(1)} INT"); raise SystemExit
    m = re.match(r"^Double\((-?[\d.]+)\)$", text)
    if m: print(f"{trim(m.group(1))} DOUBLE"); raise SystemExit
    m = re.match(r"^Matrix\(Matrix \{ xx: (\S+), xy: (\S+), yx: (\S+), yy: (\S+) \}\)$", text)
    if m:
        a, b, c, d = (trim(g.rstrip(",")) for g in m.groups())
        print(f"[{a} {b} ; {c} {d}] MATRIX"); raise SystemExit
print(text)
PY
}

# `<edit>` expressions, which no harness reached before. The value is read
# back off the substituted pattern rather than treated as a yes/no, since what
# is being compared is the value and its *type*.
edit_case() { # $1 = edit body, $2 = property, $3 = label
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?>
<fontconfig>
<match target="pattern">
  <edit name="$2" mode="assign">$1</edit>
</match>
</fontconfig>
XML
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c -d serif 2>/dev/null            | sed -n "s/^[[:space:]]*$2: //p" | head -1)
  ours=$(cargo run -q --release --example fc_match --            --config "$D/f.conf" --dump-query serif 2>/dev/null            | sed -n "s/^$2	//p" | head -1 | cut -f1)
  # Both sides say the same thing in different notation, so compare the
  # normalised form: the number and whether it is an integer or a double.
  # The two print the same value in different notation, so both are reduced
  # to "<number> <TYPE>" before comparing -- the type is half the point here,
  # since an integral result is an Integer to fontconfig and that is invisible
  # in the number alone.
  theirs=$(normalise theirs "$theirs")
  ours=$(normalise ours "$ours")
  if [ "$ours" = "$theirs" ]; then
    printf '  %-46s %-22s MATCH
' "$3" "$theirs"
  else
    printf '  %-46s ours=[%s] theirs=[%s] DIFF
' "$3" "$ours" "$theirs"
    fail
  fi
}

echo "=== <edit> expressions: arithmetic result types"
edit_case '<times><double>12.5</double><int>2</int></times>' pixelsize "times(12.5,2) is an integer"
edit_case '<divide><int>4</int><int>2</int></divide>' pixelsize "divide(4,2) is an integer"
edit_case '<divide><int>5</int><int>2</int></divide>' pixelsize "divide(5,2) is a double"
edit_case '<plus><int>1</int><int>2</int></plus>' pixelsize "plus(1,2)"
edit_case '<plus><double>1.5</double><double>2.25</double></plus>' pixelsize "plus(1.5,2.25)"
edit_case '<minus><int>10</int><double>0.5</double></minus>' pixelsize "minus(10,0.5)"

echo "=== <edit> expressions: matrices (FcMatrixMultiply)"
SHEAR='<matrix><double>1</double><double>0.2</double><double>0</double><double>1</double></matrix>'
edit_case "$SHEAR" matrix "a matrix assigned as it is"
edit_case "<times>$SHEAR$SHEAR</times>" matrix "shear times shear"
# What 90-synthetic.conf does: the query has no matrix, so `<name>` yields
# Void, which promotes to the identity -- without that a font with no italic
# face is reported oblique and rendered upright.
edit_case "<times><name>matrix</name>$SHEAR</times>" matrix "times(name matrix, shear)"
edit_case "<times>$SHEAR<name>matrix</name></times>" matrix "times(shear, name matrix)"

# Two edits on one object in one `<match>`. A test marks a value, and upstream
# holds the value's *node*, so prepending in front of it changes nothing about
# what a later `assign` replaces. An index does not survive that.
rule_case() { # $1 = rule body, $2 = query, $3 = label, $4 = object (default family)
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?>
<fontconfig>
<match target="pattern">$1</match>
</fontconfig>
XML
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c -d "$2" 2>/dev/null | python3 scripts/lib/field.py theirs "${4:-family}")
  ours=$(cargo run -q --release --example fc_match -- \
           --config "$D/f.conf" --dump-query "$2" 2>/dev/null | python3 scripts/lib/field.py ours "${4:-family}")
  if [ "$ours" = "$theirs" ]; then
    printf '  %-46s %-26s MATCH
' "$3" "$theirs"
  else
    printf '  %-46s ours=[%s] theirs=[%s] DIFF
' "$3" "$ours" "$theirs"
    fail
  fi
}

TEST_ALPHA='<test name="family"><string>Alpha</string></test>'
echo "=== a mark names a value, not a slot"
rule_case "$TEST_ALPHA<edit name=\"family\" mode=\"prepend\"><string>Beta</string></edit><edit name=\"family\" mode=\"assign\"><string>Gamma</string></edit>" Alpha "prepend then assign"
rule_case "$TEST_ALPHA<edit name=\"family\" mode=\"append\"><string>Beta</string></edit><edit name=\"family\" mode=\"assign\"><string>Gamma</string></edit>" Alpha "append then assign"
rule_case "$TEST_ALPHA<edit name=\"family\" mode=\"prepend\"><string>B</string><string>C</string></edit><edit name=\"family\" mode=\"assign\"><string>G</string></edit>" Alpha "prepend two then assign"

# A comma list in a family test: each listed name the pattern lacks discards
# what the earlier ones matched, so the last one decides.
MULTI='<test name="family"><string>Alpha</string><string>Zeta</string></test><edit name="family" mode="append"><string>Hit</string></edit>'
echo "=== a multi-valued family test"
rule_case "$MULTI" Alpha "listed Alpha,Zeta -- query Alpha"
rule_case "$MULTI" Zeta "listed Alpha,Zeta -- query Zeta"
rule_case "$MULTI" "Alpha,Zeta" "listed Alpha,Zeta -- query both"
rule_case '<test name="style"><string>A</string><string>B</string></test><edit name="family" mode="append"><string>Hit</string></edit>' "Q:style=A" "the same list on a non-family object"

# Which value a multi-valued test marks. `FcConfigMatchValueList` nests the
# loops with the *expressions* outside and the pattern's values inside, and
# `if (!ret) ret = v` freezes the mark once any expression has set one. So the
# mark follows the order the test lists its values in, not the order the query
# lists its own -- and marking the first query value that matched anything,
# which is the obvious reading, lands somewhere else whenever the two orders
# disagree.
APPEND2='<test name="family"><string>Alpha</string><string>Zeta</string></test><edit name="family" mode="append"><string>Hit</string></edit>'
echo "=== which value a multi-valued test marks"
rule_case "$APPEND2" "Zeta,Alpha" "listed Alpha,Zeta -- query Zeta,Alpha"
rule_case "$APPEND2" "Alpha,Zeta" "listed Alpha,Zeta -- query Alpha,Zeta"
# Three listed families where the middle one is absent, so the table reset
# clears a mark the first one had already set and the third sets it again.
rule_case '<test name="family"><string>Alpha</string><string>Zeta</string><string>Beta</string></test><edit name="family" mode="assign"><string>Hit</string></edit>' "Alpha,Beta,Omega" "a reset in the middle moves the mark"
rule_case '<test name="style"><string>SA</string><string>SB</string></test><edit name="style" mode="append"><string>Hit</string></edit>' "Q:style=SB:style=SA" "the same, on an object with no family table" style

# `first` and `not_first` are not part of the scan: upstream runs the ordinary
# one and then requires the mark to be, or not be, the head of the list. That
# differs from "does value 0 match" and "does any later value match" as soon
# as a test lists more than one value, or the query repeats one.
echo "=== qual=first and qual=not_first are a test on the mark"
rule_case '<test name="style" qual="not_first"><string>SA</string></test><edit name="style" mode="append"><string>Hit</string></edit>' "Q:style=SA:style=SA" "not_first, query repeats the value" style
rule_case '<test name="style" qual="not_first"><string>SA</string></test><edit name="style" mode="append"><string>Hit</string></edit>' "Q:style=SB:style=SA" "not_first, value only later" style
rule_case '<test name="style" qual="first"><string>SA</string><string>SB</string></test><edit name="style" mode="append"><string>Hit</string></edit>' "Q:style=SB:style=SA" "first, a later value marks it" style
rule_case '<test name="style" qual="first"><string>SA</string><string>SB</string></test><edit name="style" mode="append"><string>Hit</string></edit>' "Q:style=SA:style=SB" "first, the head marks it" style

# A value an `<edit>` cannot store. `FcConfigAdd` walks the whole list first
# and adds *nothing* if any of it is a type the property will not hold -- but
# `FcOpAssign` deletes the marked value afterwards either way, and
# `FcOpAssignReplace` deletes everything before it tries. So a bad value in an
# assign does not leave the property alone: it empties it.
W='<test name="weight"><int>200</int></test>'
echo "=== a value an <edit> cannot store"
rule_case "$W<edit name=\"weight\" mode=\"assign\"><int>100</int></edit>" ":weight=200" "assign, one valid value" weight
rule_case "$W<edit name=\"weight\" mode=\"assign\"><string>x</string></edit>" ":weight=200" "assign, one value of the wrong type" weight
rule_case "$W<edit name=\"weight\" mode=\"assign\"><string>x</string><int>100</int></edit>" ":weight=200" "assign, wrong type then right" weight
rule_case "$W<edit name=\"weight\" mode=\"assign\"><int>100</int><string>x</string></edit>" ":weight=200" "assign, right type then wrong" weight
rule_case "$W<edit name=\"weight\" mode=\"append\"><string>x</string><int>100</int></edit>" ":weight=200" "append, wrong type then right" weight
rule_case "$W<edit name=\"weight\" mode=\"assign_replace\"><string>x</string></edit>" ":weight=200" "assign_replace, wrong type" weight
rule_case "<edit name=\"weight\" mode=\"assign\"><string>x</string></edit>" ":weight=200" "assign with no test at all" weight
rule_case "$W<edit name=\"family\" mode=\"assign\"><int>7</int></edit>" ":weight=200" "an int assigned to family" family

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
