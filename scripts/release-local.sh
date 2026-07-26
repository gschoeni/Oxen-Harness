#!/usr/bin/env bash
# Build the CLI and/or the desktop app from the working tree and install them
# for the current user — no version bump, no GitHub, nothing pushed. The
# "I want what's on this machine, on this machine" release.
#
# Usage: scripts/release-local.sh [cli|app|all]     (default: all)
#
# - cli → release build of the `oxen-harness` binary, copied (not symlinked,
#         so later cargo builds don't mutate it) into ~/.local/bin, or
#         $OXEN_HARNESS_BIN_DIR if set. An existing symlink there is replaced.
# - app → `pnpm tauri build`; on macOS the bundle is installed to
#         /Applications/oxen-harness.app and re-registered with Launch
#         Services (so `oxen-harness ui` finds it by bundle id). On other
#         platforms the bundle paths are printed instead.
# - tag → a local-only annotated tag `local-YYYYMMDD-HHMMSS` recording the
#         installed versions and commit. Skipped when the tree is dirty
#         (a tag on a commit wouldn't describe what was actually built).
set -euo pipefail

component="${1:-all}"
case "$component" in cli | app | all) ;; *)
  echo "usage: $0 [cli|app|all]" >&2
  exit 2
  ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

installed=()

if [[ "$component" != "app" ]]; then
  echo "==> building CLI (release)"
  cargo build --release -p harness-cli
  bin_dir="${OXEN_HARNESS_BIN_DIR:-$HOME/.local/bin}"
  mkdir -p "$bin_dir"
  # rm first: `install` over a symlink would write through to its target.
  rm -f "$bin_dir/oxen-harness"
  install -m 755 target/release/oxen-harness "$bin_dir/oxen-harness"
  installed+=("cli v$(scripts/version.sh cli) -> $bin_dir/oxen-harness")
  resolved="$(command -v oxen-harness || true)"
  if [[ -n "$resolved" && "$resolved" != "$bin_dir/oxen-harness" ]]; then
    echo "warning: \`oxen-harness\` on PATH resolves to $resolved, which shadows the installed copy" >&2
  fi
fi

if [[ "$component" != "cli" ]]; then
  echo "==> building desktop app (release bundle)"
  (cd app && pnpm tauri build)
  if [[ "$(uname)" == "Darwin" ]]; then
    ditto app/src-tauri/target/release/bundle/macos/oxen-harness.app \
      /Applications/oxen-harness.app
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
      -f /Applications/oxen-harness.app
    installed+=("app v$(scripts/version.sh app) -> /Applications/oxen-harness.app")
  else
    echo "bundles under app/src-tauri/target/release/bundle/ — install with your package manager:"
    find app/src-tauri/target/release/bundle -maxdepth 2 -type f \
      \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.msi' -o -name '*.exe' \) 2>/dev/null || true
    installed+=("app v$(scripts/version.sh app) (bundle built, not auto-installed on this OS)")
  fi
fi

echo
if git diff-index --quiet HEAD --; then
  tag="local-$(date +%Y%m%d-%H%M%S)"
  git tag -a "$tag" -m "local install at $(git rev-parse --short HEAD): ${installed[*]}"
  echo "tagged $tag (local only — never pushed by the release scripts)"
else
  echo "note: working tree is dirty — skipped the local tag (it wouldn't describe what was built)"
fi

echo "installed:"
printf '  %s\n' "${installed[@]}"
