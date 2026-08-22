#!/bin/bash
# Widen the WSL font set so the parity harnesses cover more of the format.
#
# The interesting additions are not "more Latin fonts": they are the ones
# that exercise properties nothing currently installed has -- bitmap fonts
# for scalable=false and outline=false, colour emoji for color=true, Type1
# and CFF faces for fontformat variety, and more scripts for langset breadth.
set -uo pipefail

PKGS=(
  # Serif and mono counterparts to what is already here.
  dejavu-serif-fonts dejavu-fonts-all
  liberation-serif-fonts liberation-mono-fonts liberation-fonts
  gnu-free-fonts-common gnu-free-serif-fonts gnu-free-sans-fonts gnu-free-mono-fonts
  # Type1 / CFF, and a different foundry.
  urw-base35-fonts
  # Colour emoji: the only source of color=true.
  google-noto-emoji-color-fonts google-noto-emoji-fonts
  # Bitmap faces: the only source of scalable=false / outline=false.
  xorg-x11-fonts-misc terminus-fonts
  # More scripts, for langset and charset breadth.
  google-noto-sans-hebrew-fonts google-noto-sans-thai-fonts
  google-noto-sans-devanagari-fonts google-noto-sans-tamil-fonts
  google-noto-sans-georgian-fonts google-noto-sans-armenian-fonts
  google-noto-sans-ethiopic-fonts google-noto-sans-khmer-fonts
  google-noto-serif-fonts google-noto-sans-mono-fonts
  # A variable font from a different vendor.
  adobe-source-code-pro-fonts adobe-source-sans-pro-fonts
)

echo "=== before ==="
fc-list | wc -l

for p in "${PKGS[@]}"; do
  if sudo -n dnf install -y "$p" > /dev/null 2>&1; then
    echo "  installed $p"
  else
    echo "  skipped   $p"
  fi
done

sudo -n fc-cache -f > /dev/null 2>&1
fc-cache -f > /dev/null 2>&1

echo "=== after ==="
fc-list | wc -l
echo "families: $(fc-list --format='%{family}\n' | sed 's/,.*//' | sort -u | wc -l)"
echo "non-scalable: $(fc-list --format='%{scalable}\n' | grep -c False)"
echo "colour: $(fc-list --format='%{color}\n' | grep -c True)"
echo "formats: $(fc-list --format='%{fontformat}\n' | sort -u | tr '\n' ' ')"
