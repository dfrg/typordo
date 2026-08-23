#!/bin/bash
# Every parity harness, one after another, reporting only the verdicts.
#
# Run from WSL: bash /mnt/c/Work/play/fontconf/scripts/all_parity.sh
cd /mnt/c/Work/play/fontconf
for s in parity select_parity match_parity prepare_parity sort_parity lang_parity charset_parity write_parity; do
  echo "########## $s"
  bash "scripts/$s.sh" 2>&1 | grep -iE 'parity:|MATCH|DIFF|identical|===' | grep -vE '^[<>]' | head -20
done
