cd /mnt/c/work/play/fontconf
export CARGO_TARGET_DIR=/tmp/fct PATH="$HOME/.cargo/bin:$PATH"
cargo build -q --release --example fc_list || exit 1
D=$(mktemp -d); mkdir -p "$D/cache"
for spell in bold Bold BOLD nosuchconst; do
  cat > "$D/f.conf" <<XML
<?xml version="1.0"?>
<fontconfig>
<dir>/usr/share/fonts</dir>
<cachedir>$D/cache</cachedir>
<selectfont><rejectfont><pattern>
  <patelt name="weight"><const>$spell</const></patelt>
</pattern></rejectfont></selectfont>
</fontconfig>
XML
  t=$(FONTCONFIG_FILE="$D/f.conf" fc-list --format='%{file}\n' 2>/dev/null | sort -u | grep -c .)
  o=$(cargo run -q --release --example fc_list -- --config "$D/f.conf" --format file 2>/dev/null | sort -u | grep -c .)
  [ "$t" = "$o" ] && v=MATCH || v=DIFF
  printf '  <const>%-12s</const> ours=%-6s theirs=%-6s %s\n' "$spell" "$o" "$t" "$v"
done
rm -rf "$D"
