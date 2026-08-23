#!/bin/bash
# Every parity harness, one after another, reporting only the verdicts.
#
# Run: bash scripts/all_parity.sh
cd "$(dirname "$0")/.." || exit 1
for s in parity select_parity match_parity prepare_parity sort_parity lang_parity charset_parity name_parity scan_parity write_parity; do
  echo "########## $s"
  bash "scripts/$s.sh" 2>&1 | grep -iE 'parity:|MATCH|DIFF|identical|===' | grep -vE '^[<>]' | head -30
done
