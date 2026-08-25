#!/bin/bash
# Which malformed configuration values fail the load, and which are tolerated?
#
# Fontconfig grades them. `FcParseInt` and `FcParseDouble` raise
# `FcSevereError` for a body `strtol`/`strtod` will not fully consume, and a
# severe error sets `parse->error` and fails the whole configuration -- so one
# bad number anywhere discards every rule in the tree and fontconfig runs on
# its built-in configuration instead. `FcParseRange`, `FcParseMatrix` and a
# `<charset>` holding a non-integer are the same. But `<bool>` accepts any
# word and quietly means false, an out-of-range codepoint in a `<charset>` is
# only a warning, and an unknown `<const>` is a warning too.
#
# The difference is invisible in any one field: a rejected configuration still
# produces a perfectly good font list, just not the one the file asked for.
#
# Run: bash scripts/malformed_parity.sh
set -uo pipefail

FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2; exit 1 ;;
esac
cargo build -q --release --example fc_match || exit 1
MATCH="$CARGO_TARGET_DIR/release/examples/fc_match"
command -v fc-pattern >/dev/null || { echo "fc-pattern not found"; exit 1; }

D=$(mktemp -d) || exit 1
trap 'rm -rf "$D"' EXIT
mkdir -p "$D/fonts"
FONT=$(fc-match -f '%{file}' 'DejaVu Sans' 2>/dev/null)
[ -n "$FONT" ] && cp "$FONT" "$D/fonts/"

# Every config carries the same marker rule. If it survives, the file loaded;
# if the answer is the system fallback's instead, the file was rejected.
conf() { # $1 = the body under test
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?><!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig><dir>$D/fonts</dir><cachedir>$D/cache</cachedir>
<match target="pattern"><test name="family"><string>Probe</string></test>
<edit name="family" mode="append"><string>Loaded</string></edit></match>
$1
</fontconfig>
XML
}

CASES=0
DIFFS=0
# What is compared is the *verdict*: did the configuration under test load, or
# did the implementation give up on it. Not the resulting font list -- when a
# configuration is refused the two fall back to different things, and which
# built-in configuration each one reaches for is a separate question, which
# `include_parity` covers.
#
# The marker rule is the signal: it fires for every configuration that loads,
# so its absence means the file was refused.
verdict() {
  local families
  families=$(cat)
  case "$families" in
    *Loaded*) echo loaded ;;
    *) echo refused ;;
  esac
}

run() { # $1 = label
  local theirs ours
  theirs=$(FONTCONFIG_FILE="$D/f.conf" fc-pattern -c Probe 2>/dev/null </dev/null \
           | python3 scripts/lib/field.py theirs family | verdict)
  ours=$("$MATCH" --config "$D/f.conf" --dump-query Probe 2>/dev/null </dev/null \
         | python3 scripts/lib/field.py ours family | verdict)
  CASES=$((CASES + 1))
  if [ "$theirs" = "$ours" ]; then
    printf '  %-44s MATCH  (%s)\n' "$1" "$theirs"
  else
    printf '  %-44s DIFF   fc=%-10s ours=%s\n' "$1" "$theirs" "$ours"
    DIFFS=$((DIFFS + 1))
    fail
  fi
}

# A `<patelt>` is the shortest way to put a value element in a config.
sel() { conf "<selectfont><rejectfont><pattern><patelt name=\"$1\">$2</patelt></pattern></rejectfont></selectfont>"; }

sel weight '<int>200</int>';                       run "a valid int"
sel weight '<int>  200  </int>';                   run "an int padded with spaces"
sel weight '<int>notanumber</int>';                run "an int that is not a number"
sel weight '<int></int>';                          run "an empty int"
sel weight '<int>0x4e00</int>';                    run "an int in hex"
sel size '<double>12.5</double>';                  run "a valid double"
sel size '<double>abc</double>';                   run "a double that is not a number"
sel size '<double>  12.5  </double>';              run "a double padded with spaces"
sel weight '<range><int>50</int><int>200</int></range>'; run "a valid range"
sel weight '<range><int>50</int></range>';         run "a range with one bound"
sel weight '<range><int>200</int><int>50</int></range>'; run "an inverted range"
sel matrix '<matrix><double>1</double><double>0</double><double>0</double><double>1</double></matrix>'
run "a valid matrix"
sel matrix '<matrix><double>1</double><double>0</double></matrix>'; run "a matrix with two values"
sel charset '<charset><int>65</int></charset>';    run "a valid charset"
sel charset '<charset><string>A</string></charset>'; run "a charset holding a string"
sel charset '<charset><int>1114112</int></charset>'; run "a charset codepoint out of range"
sel scalable '<bool>true</bool>';                  run "a valid bool"
sel scalable '<bool>bogus</bool>';                 run "a bool that is not a spelling"
sel scalable '<bool>  true  </bool>';              run "a bool padded with spaces"
sel weight '<const>bold</const>';                  run "a known const"
sel weight '<const>nosuchconstant</const>';        run "an unknown const"
sel lang '<langset><string>ja</string></langset>'; run "a valid langset"
sel lang '<langset><int>3</int></langset>';        run "a langset holding an int"
sel lang '<langset>ja</langset>';                  run "a langset of bare text, no <string>"
sel charset '<charset></charset>';                 run "an empty charset"
sel lang '<langset></langset>';                    run "an empty langset"
sel weight '<range><int>50</int><int>100</int><int>150</int></range>'
run "a range with three bounds"
conf '<match target="pattern"><edit name="weight"><int>notanumber</int></edit></match>'
run "a bad int in an <edit>, not a <patelt>"
conf '<match target="pattern"><test name="weight"><double>abc</double></test></match>'
run "a bad double in a <test>"

echo
echo "malformed parity: $((CASES - DIFFS))/$CASES configurations agree"
exit $((FAILURES > 0))
