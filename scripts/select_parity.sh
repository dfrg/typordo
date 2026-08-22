#!/bin/bash
# Verify <selectfont> against real fontconfig.
#
# The host system has no selectfont rules, so the only way to check this
# against fc-list rather than against our own fixtures is to hand both
# implementations the same synthetic config over the real font set.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/select_parity.sh
set -uo pipefail
cd /mnt/c/Work/play/fontconf
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/fct"
cargo build -q --example fc_list || exit 1

CONF=/tmp/fontconf-select
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
  ours=$(cargo run -q --example fc_list -- --config "$conf" --format file | sort -u)
  theirs=$(FONTCONFIG_FILE="$conf" fc-list --format='%{file}\n' | sort -u)
  local no=$(echo "$ours" | grep -c . ) nt=$(echo "$theirs" | grep -c .)
  if [ "$ours" = "$theirs" ]; then
    echo "MATCH   $name: $nt files"
  else
    echo "DIFF    $name: ours=$no fc-list=$nt"
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
