#!/usr/bin/env bash
# Regenerate the desktop app's icon set from assets/app-icon.png (square,
# full-bleed artwork — no baked-in corner rounding).
#
# Two passes, because platforms disagree about shape: Windows/Linux icons are
# full-bleed squares, while macOS expects a squircle inset on a transparent
# margin (Apple's icon grid: an 824px content box on a 1024px canvas, corner
# radius ~185px). The full set is generated from the master, then icon.icns
# is regenerated from a rounded + padded variant and layered over it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

command -v magick >/dev/null || {
  echo "error: needs ImageMagick (brew install imagemagick)" >&2
  exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

size="$(magick identify -format %w assets/app-icon.png)"
content=$((size * 824 / 1024))
radius=$((content * 185 / 824))
magick assets/app-icon.png \
  -resize "${content}x${content}" \
  \( -size "${content}x${content}" xc:none \
     -fill white \
     -draw "roundrectangle 0,0,$((content - 1)),$((content - 1)),${radius},${radius}" \) \
  -compose DstIn -composite \
  -compose Over \
  -background none -gravity center -extent "${size}x${size}" \
  "$tmp/macos.png"

(cd app && pnpm tauri icon ../assets/app-icon.png)
(cd app && pnpm tauri icon -o "$tmp/out" "$tmp/macos.png")
cp "$tmp/out/icon.icns" app/src-tauri/icons/icon.icns

echo "Icon set regenerated (full-bleed everywhere, inset icon.icns for macOS)."
