#!/bin/bash
# Widen the WSL font set so the parity harnesses cover more of the format.
#
# The interesting additions are not "more Latin fonts". They are the ones
# that exercise something nothing installed has: colour emoji for
# `color=true`, bitmap faces for `scalable=false` and `outline=false`, Type 1
# and CFF for format variety, and as many scripts as can be had for langset
# and charset breadth.
#
# Two stages, because they carry different risk:
#
#   scalable   Everything the scanner already claims to handle. This should
#              widen the corpus without changing any verdict.
#   bitmap     PCF and BDF faces, which come through FreeType in fontconfig
#              and have no reader here at all. Expect scanning to disagree,
#              and read scripts/scan_parity.sh to find out how much.
#
# Run from WSL: bash scripts/install_fonts.sh [scalable|bitmap|all]
set -uo pipefail
export PATH="/usr/bin:/bin:/usr/sbin:$PATH"
stage="${1:-scalable}"

SCALABLE=(
  # Serif and mono counterparts to what is already here.
  dejavu-serif-fonts liberation-serif-fonts liberation-mono-fonts
  gnu-free-serif-fonts gnu-free-sans-fonts gnu-free-mono-fonts
  # Type 1 and CFF, and a different foundry.
  urw-base35-fonts
  # Colour, which nothing else here has: CBDT strikes and a COLR variable
  # face, the only way `color=true` is ever reached.
  google-noto-color-emoji-fonts google-noto-emoji-fonts
  google-noto-emoji-vf-fonts
  # Large CFF CJK from a second vendor, with named instances of its own.
  adobe-source-han-sans-jp-fonts adobe-source-code-pro-fonts
  adobe-source-code-vf-fonts
  # Every script Noto has a sans for. This is the point of the exercise:
  # each one is a langset and a charset the matcher has never seen.
  'google-noto-sans-*-fonts'
  'google-noto-serif-*-fonts'
)

BITMAP=(
  xorg-x11-fonts-misc xorg-x11-fonts-75dpi xorg-x11-fonts-100dpi
  terminus-fonts terminus-fonts-legacy-x11 bitmap-fonts-all
)

install() {
  echo "installing $# package patterns"
  # --skip-unavailable: the lists are deliberately optimistic, and a
  # package that moved between releases should not stop the rest.
  sudo dnf install -y --skip-unavailable --setopt=install_weak_deps=False "$@" 2>&1 | tail -3
}

before=$(fc-list | wc -l)
case "$stage" in
  scalable) install "${SCALABLE[@]}" ;;
  bitmap)   install "${BITMAP[@]}" ;;
  all)      install "${SCALABLE[@]}" "${BITMAP[@]}" ;;
  *) echo "usage: $0 [scalable|bitmap|all]"; exit 1 ;;
esac

echo "rebuilding caches"
fc-cache -f > /dev/null 2>&1

echo
echo "patterns: $before -> $(fc-list | wc -l)"
echo "files:    $(fc-list --format='%{file}\n' | sort -u | wc -l)"
echo "families: $(fc-list --format='%{family}\n' | sed 's/,.*//' | sort -u | wc -l)"
echo "formats:"
fc-list --format='%{fontformat}\n' | sort | uniq -c | sort -rn | sed 's/^/  /'
echo "languages: $(fc-list --format='%{lang}\n' | tr '|' '\n' | sort -u | grep -c .)"
echo "colour faces: $(fc-list :color=true 2>/dev/null | wc -l)"
echo "bitmap faces: $(fc-list :scalable=false 2>/dev/null | wc -l)"
