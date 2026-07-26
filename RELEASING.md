# Releasing

oxen-harness ships two artifacts on independent release lines:

| Component | What ships | Tag format | Version source of truth |
|---|---|---|---|
| **CLI** | `oxen-harness` binary (5 platforms) | `cli-vX.Y.Z` | `[workspace.package] version` in the root `Cargo.toml` |
| **Desktop app** | Tauri installers (.dmg, .AppImage, .deb, .rpm, .msi, .exe) | `app-vX.Y.Z` | `version` in `app/src-tauri/tauri.conf.json` |

They version independently because they are separate Cargo workspaces with
separate cadences — a CLI patch shouldn't force a 30-minute, 4-platform app
rebuild, and vice versa. To ship both at once, release with the `all`
component; the two tags simply land on the same commit.

Versioning is SemVer, currently pre-1.0: bump **minor** for features or
breaking changes, **patch** for fixes.

Every crate in the root workspace inherits the workspace version
(`version.workspace = true`), so the CLI version lives in exactly one place.
The app version is canonical in `tauri.conf.json` (it's what gets baked into
the bundles); the bump script mirrors it into `app/package.json` and
`app/src-tauri/Cargo.toml` so the three never drift.

## The scripts

All in `scripts/`, all runnable locally and in CI:

- **`version.sh <cli|app>`** — prints the current version. The release
  workflows use it to verify that the tag matches the source before building.
- **`bump-version.sh <cli|app|all> <patch|minor|major|X.Y.Z>`** — rewrites the
  version files and refreshes the Cargo.lock files (`cargo update --workspace`
  re-pins only workspace members; dependencies are untouched). Edits files
  only — it never commits.
- **`release.sh <cli|app|all> [--yes]`** — creates annotated tag(s) from the
  *committed* versions and pushes them, which triggers the release
  workflow(s). Refuses to run on a dirty tree or if the tag already exists
  locally or on origin.

## Release path A: from the GitHub Actions UI (one button)

1. Go to **Actions → Cut Release → Run workflow**.
2. Pick the component (`cli`, `app`, or `all`) and the bump (`patch`, `minor`,
   `major`), or type an explicit version to override the bump.
3. Run it. The workflow bumps the version(s), commits `release: cli vX.Y.Z` to
   `main`, pushes the tag(s), and invokes the release build(s) directly.

That last step matters: pushes made with the workflow's `GITHUB_TOKEN` do
**not** trigger tag-push workflows (GitHub's recursion guard), so `cut-release`
calls `release-cli.yml` / `release-app.yml` as reusable workflows instead of
relying on the tag event. If `main` has branch protection that blocks the
github-actions bot, use path B.

## Local release: just this machine

For "I want what's on this machine, on this machine" — no bump, no tag push,
no GitHub:

```bash
scripts/release-local.sh          # or: cli / app to do only one
```

It release-builds the CLI and copies it to `~/.local/bin/oxen-harness`
(`$OXEN_HARNESS_BIN_DIR` overrides; an existing symlink there is replaced by
a real copy, so later `cargo build`s don't mutate the installed command),
builds the app bundle and installs it to `/Applications/oxen-harness.app`
(macOS; registered with Launch Services so `oxen-harness ui` finds it), and —
when the tree is clean — drops a local-only annotated tag
`local-YYYYMMDD-HHMMSS` recording exactly what was installed. Those tags are
never pushed by the release scripts; they're your machine's install history
(`git tag -l 'local-*' -n1`).

## Release path B: from your machine

```bash
# 1. Bump — edits the version files + lockfiles, nothing else.
scripts/bump-version.sh cli patch      # or: app minor, all 0.3.0, ...

# 2. Review and commit the bump.
git diff
git commit -am "release: cli v0.1.1"

# 3. Push the commit, then tag it. release.sh shows you what it's about to
#    tag and asks before pushing; the pushed tag triggers the release build.
git push
scripts/release.sh cli
```

Because you push the tag with your own credentials, the tag-push trigger fires
normally — no extra step.

## What the workflows do

**`release-cli.yml`** (on `cli-v*` tags):

1. **verify** — checks out the tag and fails unless the tag equals
   `cli-v$(scripts/version.sh cli)`. No mismatched binaries, ever.
2. **build** — matrix over five targets: Linux x86_64 + aarch64 (native ARM
   runner), macOS arm64 + x86_64 (cross-compiled on Apple Silicon), Windows
   x86_64. Each produces `oxen-harness-<version>-<target>.tar.gz` (`.zip` on
   Windows) containing the binary, LICENSE, and README, plus a `.sha256`.
3. **release** — publishes a GitHub release with auto-generated notes, all
   archives, per-file checksums, and a combined `SHA256SUMS`.

**`release-app.yml`** (on `app-v*` tags):

1. **verify** — same tag/version guard against `tauri.conf.json`.
2. **build** — `tauri-apps/tauri-action` over four lanes: macOS arm64, macOS
   x86_64, Linux x86_64 (Ubuntu 22.04 for older-glibc compatibility), Windows
   x86_64. The first lane to finish creates a **draft** release on the tag;
   the rest upload their bundles to it.
3. **You publish** — once all four lanes are green, sanity-check one bundle
   and click **Publish** on the draft. The draft is deliberate: installers are
   worth a look before they're public, and it means the release appears whole
   rather than accumulating assets over ~30 minutes.

CLI releases publish automatically; only app releases stop at a draft.

One naming note: the app's installed binary is **`oxen-harness-app`**
(`mainBinaryName` in `tauri.conf.json`), not `oxen-harness` — the CLI owns
that name, and the Linux packages would otherwise collide with it on PATH.
`oxen-harness ui [dir]` relies on this name (and the `ai.oxen.harness` bundle
id on macOS) to find and launch the installed app.

## Code signing (optional, macOS)

Without secrets configured, macOS bundles are ad-hoc signed — users must
right-click → Open (or `xattr -cr`) on first launch. To sign and notarize,
add these repo secrets and the workflow picks them up automatically:
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`. Windows signing and the Tauri
updater are future work.

## When something goes wrong

- **verify failed** — the tag doesn't match the committed version. Delete the
  tag (`git push origin :refs/tags/cli-vX.Y.Z && git tag -d cli-vX.Y.Z`), fix
  the version with `bump-version.sh`, commit, re-run `release.sh`.
- **One build lane failed** (flaky runner) — re-run the failed jobs from the
  Actions UI; the matrix is `fail-fast: false`, so the others are unaffected.
  For the CLI, the `release` job only runs when all lanes pass, so a re-run
  picks up where it left off. For the app, a re-run re-uploads to the same
  draft.
- **Release already exists** — `gh release create` fails rather than
  clobbering. Delete the release (keeping or deleting the tag as appropriate)
  and re-run.
- **Nothing shipped, tag is wrong** — tags are cheap before a release is
  published. Delete tag + draft and start over. Never delete a *published*
  release's tag; ship a new patch version instead.
