#!/bin/bash
# Compile for every layout fontconfig has a name for at our endianness.
#
# There is no fontconfig to compare against on these targets, so this proves
# only that the derivation in src/layout.rs holds: the const assertions there
# restate the five closed forms from fcarch.c, and a wrong offset makes them
# fail to compile. It does not prove we can read a real 32-bit cache.
#
# The two 32-bit shapes are the point: i686 aligns a double to one word
# (le32d4), 32-bit ARM to two (le32d8), and fontconfig writes different bytes
# for each.
#
# Run from anywhere: bash scripts/cross_check.sh
set -u
cd "$(dirname "$0")/.."

targets="x86_64-unknown-linux-gnu i686-unknown-linux-gnu armv7-unknown-linux-gnueabihf"
missing=""
for t in $targets; do
  rustup target list --installed | grep -qx "$t" || missing="$missing $t"
done
if [ -n "$missing" ]; then
  echo "missing targets:$missing"
  echo "  rustup target add$missing"
  exit 1
fi

status=0
for t in $targets; do
  printf '%-36s ' "$t"
  if out=$(cargo check -q --target "$t" --features scan 2>&1) && [ -z "$out" ]; then
    echo "ok"
  else
    echo "FAILED"
    echo "$out" | head -15
    status=1
  fi
done
exit $status
