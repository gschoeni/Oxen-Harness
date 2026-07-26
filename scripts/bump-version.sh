#!/usr/bin/env bash
# Bump the version of the CLI, the desktop app, or both.
#
# Usage: scripts/bump-version.sh <cli|app|all> <patch|minor|major|X.Y.Z>
#
# cli — rewrites [workspace.package] version in the root Cargo.toml (every
#       crate inherits it) and refreshes the root Cargo.lock.
# app — rewrites app/src-tauri/tauri.conf.json (canonical), app/package.json,
#       and app/src-tauri/Cargo.toml, and refreshes the app Cargo.lock.
#
# Only edits files; committing and tagging are release.sh's job.
set -euo pipefail

usage() {
  echo "usage: $0 <cli|app|all> <patch|minor|major|X.Y.Z>" >&2
  exit 2
}

component="${1:-}"
spec="${2:-}"
case "$component" in cli | app | all) ;; *) usage ;; esac
[[ -n "$spec" ]] || usage

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

python3 - "$component" "$spec" <<'PY'
import json, re, sys
from pathlib import Path

component, spec = sys.argv[1], sys.argv[2]

def next_version(cur: str, spec: str) -> str:
    if re.fullmatch(r"\d+\.\d+\.\d+", spec):
        return spec
    major, minor, patch = map(int, cur.split("."))
    if spec == "major":
        return f"{major + 1}.0.0"
    if spec == "minor":
        return f"{major}.{minor + 1}.0"
    if spec == "patch":
        return f"{major}.{minor}.{patch + 1}"
    sys.exit(f"unknown bump '{spec}' (want patch, minor, major, or X.Y.Z)")

def set_toml_version(path: str, new: str) -> None:
    # Only the package/workspace version sits at column 0; dependency
    # versions are all inline-table or `name = "x"` entries.
    p = Path(path)
    text, n = re.subn(r'(?m)^version = ".*"$', f'version = "{new}"', p.read_text(), count=1)
    if n != 1:
        sys.exit(f"{path}: no `version = \"...\"` line found")
    p.write_text(text)

def set_json_version(path: str, new: str) -> None:
    p = Path(path)
    data = json.loads(p.read_text())
    data["version"] = new
    p.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")

if component in ("cli", "all"):
    cur = re.search(r'(?m)^version = "(.*)"$', Path("Cargo.toml").read_text()).group(1)
    new = next_version(cur, spec)
    set_toml_version("Cargo.toml", new)
    print(f"cli: {cur} -> {new}")

if component in ("app", "all"):
    cur = json.loads(Path("app/src-tauri/tauri.conf.json").read_text())["version"]
    new = next_version(cur, spec)
    set_json_version("app/src-tauri/tauri.conf.json", new)
    set_json_version("app/package.json", new)
    set_toml_version("app/src-tauri/Cargo.toml", new)
    print(f"app: {cur} -> {new}")
PY

# Sync the new versions into the lockfiles without touching dependency
# versions (`--workspace` only re-pins workspace members).
if command -v cargo >/dev/null; then
  if [[ "$component" != "app" && -f Cargo.lock ]]; then
    cargo update --workspace --quiet
  fi
  if [[ "$component" != "cli" && -f app/src-tauri/Cargo.lock ]]; then
    cargo update --workspace --quiet --manifest-path app/src-tauri/Cargo.toml
  fi
else
  echo "warning: cargo not found; Cargo.lock files not refreshed" >&2
fi
