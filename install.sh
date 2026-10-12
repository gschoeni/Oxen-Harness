#!/bin/sh
# Install the oxen-harness CLI and desktop app from the latest GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/gschoeni/Oxen-Harness/main/install.sh | sh
#
# Options (pass after `sh -s --`):
#   --cli-only   install only the `oxen-harness` CLI
#   --app-only   install only the desktop app
#
# Environment:
#   OXEN_HARNESS_BIN_DIR   where binaries go (default: ~/.local/bin)
#   OXEN_HARNESS_APP_DIR   macOS: where the .app goes (default: /Applications,
#                          or ~/Applications when that isn't writable)
#   GITHUB_TOKEN           optional, avoids GitHub API rate limits
#
# The CLI and the desktop app release on independent tags (cli-vX.Y.Z and
# app-vX.Y.Z), so "latest" is resolved per component rather than through
# GitHub's single /releases/latest.

set -eu

REPO="gschoeni/Oxen-Harness"
API="https://api.github.com/repos/$REPO"
DOWNLOADS="https://github.com/$REPO/releases/download"

want_cli=1
want_app=1
for arg in "$@"; do
  case "$arg" in
    --cli-only) want_app=0 ;;
    --app-only) want_cli=0 ;;
    -h|--help)
      echo "usage: install.sh [--cli-only | --app-only]"
      echo "env: OXEN_HARNESS_BIN_DIR, OXEN_HARNESS_APP_DIR, GITHUB_TOKEN"
      exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ -t 1 ]; then
  bold=$(printf '\033[1m'); green=$(printf '\033[32m'); yellow=$(printf '\033[33m')
  red=$(printf '\033[31m'); reset=$(printf '\033[0m')
else
  bold=""; green=""; yellow=""; red=""; reset=""
fi
say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$bold" "$reset" "$*"; }
warn() { printf '%swarning:%s %s\n' "$yellow" "$reset" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$red" "$reset" "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "this installer needs \`$1\`"; }
need curl
need tar
need uname

fetch() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$@"
  else
    curl -fsSL "$@"
  fi
}

# --- platform -------------------------------------------------------------

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin) os=macos ;;
  Linux)  os=linux ;;
  MINGW*|MSYS*|CYGWIN*)
    die "on Windows, download the installer from https://github.com/$REPO/releases" ;;
  *) die "unsupported OS: $os" ;;
esac
case "$arch" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) die "unsupported architecture: $arch" ;;
esac

# Rosetta reports x86_64 from an arm64 Mac; prefer the native build.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] \
   && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch=aarch64
fi

case "$os-$arch" in
  macos-aarch64) cli_target=aarch64-apple-darwin ;;
  macos-x86_64)  cli_target=x86_64-apple-darwin ;;
  linux-aarch64) cli_target=aarch64-unknown-linux-gnu ;;
  linux-x86_64)  cli_target=x86_64-unknown-linux-gnu ;;
esac

bin_dir="${OXEN_HARNESS_BIN_DIR:-$HOME/.local/bin}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

# --- release lookup -------------------------------------------------------

releases_json=""
# Newest published tag with the given prefix (the API lists newest first and
# hides drafts from unauthenticated callers).
latest_tag() {
  if [ -z "$releases_json" ]; then
    releases_json=$(fetch "$API/releases?per_page=50") \
      || die "could not reach the GitHub API (set GITHUB_TOKEN if rate-limited)"
  fi
  printf '%s\n' "$releases_json" \
    | grep -o "\"tag_name\": *\"$1[^\"]*\"" \
    | head -n 1 \
    | sed 's/.*"\([^"]*\)"$/\1/'
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# --- CLI ------------------------------------------------------------------

install_cli() {
  tag=$(latest_tag cli-v)
  [ -n "$tag" ] || die "no CLI release found"
  version=${tag#cli-v}
  name="oxen-harness-$version-$cli_target"
  step "Installing the oxen-harness CLI $version ($cli_target)"

  fetch -o "$tmp/$name.tar.gz" "$DOWNLOADS/$tag/$name.tar.gz" \
    || die "download failed: $DOWNLOADS/$tag/$name.tar.gz"
  fetch -o "$tmp/$name.tar.gz.sha256" "$DOWNLOADS/$tag/$name.tar.gz.sha256" \
    || die "checksum download failed"
  expected=$(cut -d' ' -f1 < "$tmp/$name.tar.gz.sha256")
  actual=$(sha256_of "$tmp/$name.tar.gz")
  [ "$expected" = "$actual" ] || die "checksum mismatch for $name.tar.gz"

  tar xzf "$tmp/$name.tar.gz" -C "$tmp"
  mkdir -p "$bin_dir"
  # Replace via rename so a running copy of the old binary isn't clobbered.
  cp "$tmp/$name/oxen-harness" "$bin_dir/.oxen-harness.new"
  chmod 755 "$bin_dir/.oxen-harness.new"
  mv -f "$bin_dir/.oxen-harness.new" "$bin_dir/oxen-harness"
  say "    ${green}✓${reset} $bin_dir/oxen-harness"
}

# --- desktop app ----------------------------------------------------------

install_app_macos() {
  tag=$1
  case "$arch" in
    aarch64) asset=oxen-harness_aarch64.app.tar.gz ;;
    x86_64)  asset=oxen-harness_x64.app.tar.gz ;;
  esac
  fetch -o "$tmp/$asset" "$DOWNLOADS/$tag/$asset" \
    || die "download failed: $DOWNLOADS/$tag/$asset"
  mkdir -p "$tmp/app"
  tar xzf "$tmp/$asset" -C "$tmp/app"
  [ -d "$tmp/app/oxen-harness.app" ] || die "unexpected archive layout in $asset"

  app_dir="${OXEN_HARNESS_APP_DIR:-}"
  if [ -z "$app_dir" ]; then
    if [ -w /Applications ]; then app_dir=/Applications; else app_dir="$HOME/Applications"; fi
  fi
  mkdir -p "$app_dir"
  rm -rf "$app_dir/oxen-harness.app"
  mv "$tmp/app/oxen-harness.app" "$app_dir/"
  # Release builds may be ad-hoc signed; clear quarantine so Gatekeeper
  # doesn't block the first launch.
  xattr -dr com.apple.quarantine "$app_dir/oxen-harness.app" 2>/dev/null || true
  # Register the bundle id so `oxen-harness ui` (open -b) finds it right away.
  lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  [ -x "$lsregister" ] && "$lsregister" -f "$app_dir/oxen-harness.app" 2>/dev/null || true
  say "    ${green}✓${reset} $app_dir/oxen-harness.app"
}

install_app_linux() {
  tag=$1 version=$2
  if [ "$arch" != x86_64 ]; then
    warn "no desktop build for linux-$arch yet; skipping the app (the CLI works everywhere)"
    return 0
  fi
  asset="oxen-harness_${version}_amd64.AppImage"
  fetch -o "$tmp/$asset" "$DOWNLOADS/$tag/$asset" \
    || die "download failed: $DOWNLOADS/$tag/$asset"
  mkdir -p "$bin_dir"
  # `oxen-harness-app` on PATH is what `oxen-harness ui` launches on Linux.
  cp "$tmp/$asset" "$bin_dir/.oxen-harness-app.new"
  chmod 755 "$bin_dir/.oxen-harness-app.new"
  mv -f "$bin_dir/.oxen-harness-app.new" "$bin_dir/oxen-harness-app"
  say "    ${green}✓${reset} $bin_dir/oxen-harness-app (AppImage; needs libfuse2)"
}

install_app() {
  tag=$(latest_tag app-v)
  [ -n "$tag" ] || die "no desktop app release found"
  version=${tag#app-v}
  step "Installing the oxen-harness desktop app $version"
  case "$os" in
    macos) install_app_macos "$tag" ;;
    linux) install_app_linux "$tag" "$version" ;;
  esac
}

# --- main -----------------------------------------------------------------

[ "$want_cli" = 1 ] && install_cli
[ "$want_app" = 1 ] && install_app

say ""
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    if [ "$want_cli" = 1 ] || [ "$os" = linux ]; then
      warn "$bin_dir is not on your PATH. Add it with:"
      say "    echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.$(basename "${SHELL:-sh}")rc && export PATH=\"$bin_dir:\$PATH\""
      say ""
    fi
    ;;
esac

say "${bold}You're on the trail.${reset}"
say ""
say "  Save your Oxen.ai API key (https://oxen.ai) once; the CLI and the app both read it:"
say "    mkdir -p ~/.oxen-harness && echo 'OXEN_API_KEY=...' >> ~/.oxen-harness/.env"
say ""
[ "$want_cli" = 1 ] && say "  oxen-harness          # chat in the terminal"
[ "$want_cli" = 1 ] && [ "$want_app" = 1 ] && say "  oxen-harness ui .     # open the desktop app on this folder"
say ""
