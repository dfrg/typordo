#!/bin/bash
# Does a configuration load, or fall back, in the same cases?
#
# `<include>` decides more than which rules are read. `_FcConfigParse` resolves
# the name to **one** file and reads that; a required include that resolves to
# nothing fails the whole load, the including file's rules with it, and
# fontconfig then runs on its built-in configuration -- so a single missing
# file changes every answer rather than dropping one file's rules.
# `ignore_missing` suppresses all of that, a malformed file included.
#
# None of it is visible in any single field, which is why it took a re-audit:
# the comparison is between "the configuration under test loaded" and "the
# built-in fallback did", and both produce a perfectly good font list.
#
# Run: bash scripts/include_parity.sh
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
mkdir -p "$D/fonts" "$D/a" "$D/b" "$D/confd"
FONT=$(fc-match -f '%{file}' 'DejaVu Sans' 2>/dev/null)
[ -n "$FONT" ] && cp "$FONT" "$D/fonts/"

# The same file name under two search-path entries, each appending a different
# family, so which one was read is visible in the answer.
alias_conf() { # $1 = file, $2 = appended family
  cat > "$1" <<XML
<?xml version="1.0"?>
<fontconfig>
<match target="pattern"><test name="family"><string>Probe</string></test>
<edit name="family" mode="append"><string>$2</string></edit></match>
</fontconfig>
XML
}
alias_conf "$D/a/shared.conf" FromA
alias_conf "$D/b/shared.conf" FromB
alias_conf "$D/confd/50-numbered.conf" FromConfD
alias_conf "$D/confd/unnumbered.conf" FromUnnumbered
printf 'not xml at all <<<' > "$D/a/broken.conf"

conf() { # $1 = body
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?><!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig><dir>$D/fonts</dir><cachedir>$D/cache</cachedir>
$1
</fontconfig>
XML
}

CASES=0
DIFFS=0
run() { # $1 = label
  # Hashed, not printed: when a configuration fails to load, both sides answer
  # from the system fallback, whose alias list runs to several kilobytes.
  local theirs ours
  theirs=$(FONTCONFIG_PATH="$D/a:$D/b" FONTCONFIG_FILE="$D/f.conf" \
           fc-pattern -c Probe 2>/dev/null </dev/null \
           | python3 scripts/lib/field.py theirs family)
  ours=$(FONTCONFIG_PATH="$D/a:$D/b" "$MATCH" --config "$D/f.conf" \
         --dump-query Probe 2>/dev/null </dev/null \
         | python3 scripts/lib/field.py ours family)
  CASES=$((CASES + 1))
  local shown="$theirs"
  if [ "${#shown}" -gt 40 ]; then shown="${shown:0:34}... (the fallback)"; fi
  if [ "$theirs" = "$ours" ]; then
    printf '  %-40s MATCH  [%s]\n' "$1" "$shown"
  else
    printf '  %-40s DIFF\n           fc=[%.90s]\n         ours=[%.90s]\n' "$1" "$theirs" "$ours"
    DIFFS=$((DIFFS + 1))
    fail
  fi
}

conf '<include>shared.conf</include>'
run "one name, two search-path entries"
conf '<include ignore_missing="yes">nowhere.conf</include>'
run "optional, missing"
conf '<include>nowhere.conf</include>'
run "required, missing"
conf '<include ignore_missing="yes">broken.conf</include>'
run "optional, malformed"
conf '<include>broken.conf</include>'
run "required, malformed"
conf "<include>$D/a/shared.conf</include>"
run "absolute, present"
conf "<include>$D/a/nowhere.conf</include>"
run "absolute, missing"
conf "<include ignore_missing=\"yes\">$D/a/nowhere.conf</include>"
run "absolute, optional, missing"
conf "<include>$D/confd</include>"
run "a directory takes only [0-9]*.conf"
conf "<include>$D/a/shared.conf</include><include>$D/a/shared.conf</include>"
run "the same file included twice"
conf "<include prefix=\"relative\">a/shared.conf</include>"
run "prefix=relative, against the including file"

echo
echo "include parity: $((CASES - DIFFS))/$CASES configurations agree"
exit $((FAILURES > 0))
