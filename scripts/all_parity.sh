#!/bin/bash
# Every parity harness, one after another, reporting only the verdicts.
#
# Exits non-zero if any of them found a difference, so this is usable as a
# check and not only as something to read.
#
# Run: bash scripts/all_parity.sh
cd "$(dirname "$0")/.." || exit 1

FAILURES=0
for s in parity select_parity match_parity prepare_parity sort_parity \
         lang_parity charset_parity cover_parity name_parity scan_parity \
         symbol_parity write_parity compare_parity; do
  echo "########## $s"
  # Captured rather than piped: `head` closing the pipe would send SIGPIPE
  # upstream, and a harness killed by a signal is indistinguishable from one
  # that found a difference. Capturing keeps the exit code the script's own.
  out=$(bash "scripts/$s.sh" 2>&1)
  status=$?
  echo "$out" | grep -iE 'parity:|MATCH|DIFF|identical|===' | grep -vE '^[<>]' | head -30
  if [ "$status" -ne 0 ]; then
    FAILURES=$((FAILURES + 1))
    echo "  ^^ $s FAILED (exit $status)"
  fi
done

echo
if [ "$FAILURES" -gt 0 ]; then
  echo "FAILED: $FAILURES harness(es) found a difference"
else
  echo "all harnesses agree with fontconfig"
fi
exit $((FAILURES > 0))
