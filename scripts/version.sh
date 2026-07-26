#!/usr/bin/env bash
# Print the current version of a release component.
#
# Usage: scripts/version.sh <cli|app>
#
# The CLI version is the root workspace version (every crate inherits it via
# `version.workspace = true`). The app version is canonical in
# app/src-tauri/tauri.conf.json; bump-version.sh keeps the mirrors in sync.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "${1:-}" in
  cli)
    sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1
    ;;
  app)
    python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' \
      "$root/app/src-tauri/tauri.conf.json"
    ;;
  *)
    echo "usage: $0 <cli|app>" >&2
    exit 2
    ;;
esac
