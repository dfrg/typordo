#!/bin/bash
# Verify <selectfont> against real fontconfig.
#
# The host system has no selectfont rules, so the only way to check this
# against fc-list rather than against our own fixtures is to hand both
# implementations the same synthetic config over the real font set.
#
# Run: bash scripts/select_parity.sh
set -uo pipefail

# A harness is a check, not a report. Anything that differs has to make the
# script fail, or a caller running it -- CI most of all -- is told everything
# passed while it is looking at differences.
FAILURES=0
fail() { FAILURES=$((FAILURES + 1)); }
cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
# An absolute path, or cargo builds inside the repository. That is not
# hypothetical: a shell that mangled `$HOME` once handed these scripts
# `C:Userscbrok/fct`, which has no leading slash, and cargo dutifully created
# it here -- where `git add -A` then committed it. Twice.
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *)
    echo "CARGO_TARGET_DIR must be an absolute path, got: $CARGO_TARGET_DIR" >&2
    exit 1
    ;;
esac
cargo build -q --release --example fc_list || exit 1

CONF=/tmp/typordo-select
mkdir -p $CONF

write_conf() {
  cat > "$CONF/$1" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>/usr/share/fonts</dir>
  <cachedir prefix="xdg">fontconfig</cachedir>
  <cachedir>/usr/lib/fontconfig/cache</cachedir>
  <selectfont>
$2
  </selectfont>
</fontconfig>
EOF
}

check() {
  local name="$1"
  local conf="$CONF/$name"
  local ours theirs
  ours=$(cargo run -q --release --example fc_list -- --config "$conf" --format file | sort -u)
  theirs=$(FONTCONFIG_FILE="$conf" fc-list --format='%{file}\n' | sort -u)
  local no=$(echo "$ours" | grep -c . ) nt=$(echo "$theirs" | grep -c .)
  if [ "$ours" = "$theirs" ]; then
    echo "MATCH   $name: $nt files"
  else
    echo "DIFF    $name: ours=$no fc-list=$nt"
    fail
    diff <(echo "$ours") <(echo "$theirs") | head -6
  fi
}

# Baseline: same config shape, no rules at all.
write_conf baseline.conf ""
check baseline.conf

# Reject a whole family of directories by glob.
write_conf reject-glob.conf '    <rejectfont>
      <glob>*/vazirmatn*/*</glob>
    </rejectfont>'
check reject-glob.conf

# Reject by glob, then rescue one file with an accept glob.
write_conf accept-wins.conf '    <rejectfont>
      <glob>*/dejavu-sans-fonts/*</glob>
    </rejectfont>
    <acceptfont>
      <glob>*/DejaVuSans.ttf</glob>
    </acceptfont>'
check accept-wins.conf

# Reject by pattern on a property.
write_conf reject-pattern.conf '    <rejectfont>
      <pattern>
        <patelt name="family"><string>DejaVu Sans</string></patelt>
      </pattern>
    </rejectfont>'
check reject-pattern.conf

# Case and blanks in a selector string must be ignored.
write_conf reject-folded.conf '    <rejectfont>
      <pattern>
        <patelt name="family"><string>  dejavusans  </string></patelt>
      </pattern>
    </rejectfont>'
check reject-folded.conf

# A selector naming two properties matches only if both do.
write_conf reject-two.conf '    <rejectfont>
      <pattern>
        <patelt name="family"><string>DejaVu Sans</string></patelt>
        <patelt name="slant"><int>0</int></patelt>
      </pattern>
    </rejectfont>'
check reject-two.conf

# Rejecting a whole directory should prune it from the walk.
write_conf reject-dir.conf '    <rejectfont>
      <glob>/usr/share/fonts/google-noto</glob>
    </rejectfont>'
check reject-dir.conf

# <const> resolves per property: roman means slant 0.
write_conf const-slant.conf '    <rejectfont>
      <pattern>
        <patelt name="slant"><const>roman</const></patelt>
      </pattern>
    </rejectfont>'
check const-slant.conf

# <const> for weight: bold is 200.
write_conf const-weight.conf '    <rejectfont>
      <pattern>
        <patelt name="weight"><const>bold</const></patelt>
      </pattern>
    </rejectfont>'
check const-weight.conf

# The same name means different numbers for different properties.
write_conf const-normal-width.conf '    <rejectfont>
      <pattern>
        <patelt name="width"><const>normal</const></patelt>
      </pattern>
    </rejectfont>'
check const-normal-width.conf

# <charset> selects fonts covering the given codepoints. 0x4e00 is CJK.
write_conf charset-cjk.conf '    <rejectfont>
      <pattern>
        <patelt name="charset"><charset><int>0x4e00</int></charset></patelt>
      </pattern>
    </rejectfont>'
check charset-cjk.conf

# A non-ASCII fold: the sharp s folds to "ss", which ASCII lowercasing misses.
write_conf fold-sharp-s.conf '    <rejectfont>
      <pattern>
        <patelt name="family"><string>DEJAVU SANS</string></patelt>
      </pattern>
    </rejectfont>'
check fold-sharp-s.conf

# <langset> selects fonts that answer a language.
write_conf langset-en.conf '    <rejectfont>
      <pattern>
        <patelt name="lang"><langset><string>en</string></langset></patelt>
      </pattern>
    </rejectfont>'
check langset-en.conf

write_conf langset-two.conf '    <rejectfont>
      <pattern>
        <patelt name="lang"><langset><string>en</string><string>de</string></langset></patelt>
      </pattern>
    </rejectfont>'
check langset-two.conf

# A region fontconfig has no bit for. It has to match anyway: a font listing
# `en` answers a request for `en-GB`, and a bitmap alone cannot say so.
write_conf langset-region.conf '    <rejectfont>
      <pattern>
        <patelt name="lang"><langset><string>en-GB</string></langset></patelt>
      </pattern>
    </rejectfont>'
check langset-region.conf

# The same, spelled with a capital region, since language names fold.
write_conf langset-case.conf '    <rejectfont>
      <pattern>
        <patelt name="lang"><langset><string>en-US</string></langset></patelt>
      </pattern>
    </rejectfont>'
check langset-case.conf

# A language nothing has, which must reject nothing rather than everything.
write_conf langset-unknown.conf '    <rejectfont>
      <pattern>
        <patelt name="lang"><langset><string>xx-yy</string></langset></patelt>
      </pattern>
    </rejectfont>'
check langset-unknown.conf

# <range> selects a span. A font weight is a scalar for a static face and a
# range for a variable one, and both have to sit inside the span named here.
write_conf range-weight.conf '    <rejectfont>
      <pattern>
        <patelt name="weight"><range><int>0</int><int>100</int></range></patelt>
      </pattern>
    </rejectfont>'
check range-weight.conf

# A span narrow enough that a variable font cannot fit inside it, which is
# what tells "the font is within the span" apart from "they overlap".
write_conf range-narrow.conf '    <rejectfont>
      <pattern>
        <patelt name="weight"><range><int>80</int><int>80</int></range></patelt>
      </pattern>
    </rejectfont>'
check range-narrow.conf

write_conf range-double.conf '    <rejectfont>
      <pattern>
        <patelt name="weight"><range><double>0.0</double><double>200.5</double></range></patelt>
      </pattern>
    </rejectfont>'
check range-double.conf

# <range> inside <charset>: a span of codepoints rather than one.
write_conf charset-range.conf '    <rejectfont>
      <pattern>
        <patelt name="charset"><charset><range><int>0x4e00</int><int>0x4e10</int></range></charset></patelt>
      </pattern>
    </rejectfont>'
check charset-range.conf

# A codepoint and a span together, which is how the two collection paths meet.
# Both have to be assigned characters or the test proves nothing.
write_conf charset-mixed.conf '    <rejectfont>
      <pattern>
        <patelt name="charset"><charset><int>0x41</int><range><int>0x3042</int><int>0x3046</int></range></charset></patelt>
      </pattern>
    </rejectfont>'
check charset-mixed.conf

# An empty <pattern/> matches every font: `FcListPatternMatchAny` walks its
# elements and finds nothing to disagree with. So an empty <rejectfont>
# rejects everything and an empty <acceptfont> accepts everything, and getting
# this wrong inverts the rule rather than weakening it.
write_conf empty-reject.conf '    <rejectfont>
      <pattern/>
    </rejectfont>'
check empty-reject.conf

write_conf empty-accept.conf '    <acceptfont>
      <pattern/>
    </acceptfont>'
check empty-accept.conf

# `namelang` sets familylang, stylelang and fullnamelang together and never
# appears on a font, so `FcListPatternMatchAny` skips it by name -- leaving,
# here, a selector with nothing left to check, which therefore matches
# everything.
write_conf namelang-skipped.conf '    <rejectfont>
      <pattern>
        <patelt name="namelang"><string>en</string></patelt>
      </pattern>
    </rejectfont>'
check namelang-skipped.conf

# The same, with a real condition beside it: the namelang element drops out
# and the family element still has to hold.
write_conf namelang-with-family.conf '    <rejectfont>
      <pattern>
        <patelt name="namelang"><string>en</string></patelt>
        <patelt name="family"><string>DejaVu Sans</string></patelt>
      </pattern>
    </rejectfont>'
check namelang-with-family.conf

# `<bool>` is read by `FcNameBool`, where the first letter decides: true,
# yes, on and 1 are all true. A spelling it does not know is false -- not
# ignored -- so `<bool>bogus</bool>` still selects the non-scalable fonts.
for spell in true yes on 1 True false no off 0 bogus; do
  write_conf "bool-$spell.conf" "    <rejectfont>
      <pattern>
        <patelt name=\"scalable\"><bool>$spell</bool></patelt>
      </pattern>
    </rejectfont>"
  check "bool-$spell.conf"
done

# `<const>` resolves through `FcNameGetConstant`, which compares with
# `FcStrCmpIgnoreCase`, so the spelling does not matter. A name the table does
# not hold is `FcTypeVoid`, and `FcParsePatelt` stops at the first Void value
# -- so the <patelt> adds nothing, leaving an empty <pattern> that matches
# every font and rejects the lot.
for spell in bold Bold BOLD nosuchconst; do
  write_conf "const-$spell.conf" "    <rejectfont>
      <pattern>
        <patelt name=\"weight\"><const>$spell</const></patelt>
      </pattern>
    </rejectfont>"
  check "const-$spell.conf"
done

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAILED: $FAILURES difference(s) -- see above"
fi
exit $((FAILURES > 0))
