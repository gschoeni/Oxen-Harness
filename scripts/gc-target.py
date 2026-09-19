#!/usr/bin/env python3
"""Garbage-collect the cargo target directory.

Cargo never deletes anything from `target/`. Every dependency bump, feature
change, toolchain update, or profile tweak compiles a fresh set of artifacts
under new hashes and leaves the old set behind, so a long-lived checkout
accumulates tens of gigabytes of rlibs, object files, and test binaries that
no build will ever read again.

This script keeps exactly the artifacts the project's own check suite uses and
removes the rest. It learns the live set from cargo itself: each warm command
below is run with `--message-format=json`, which reports every artifact a build
produces (including fresh ones it did not need to recompile), and anything in
`target/debug` whose hash is not in that set is deleted. Deleting is always
safe — the worst case is cargo rebuilding a unit — so the script only aborts
when a warm command fails, since a partial artifact list would prune too much.

    scripts/gc-target.py            # prune
    scripts/gc-target.py --dry-run  # show what would go
    scripts/gc-target.py --days 3   # be more aggressive with incremental caches

Incremental caches and non-dev profile directories (release, cross targets)
are not in the JSON output, so they go by age instead: anything untouched for
`--days` days (default 3) is removed. Incremental caches are the largest
bucket by far on a busy checkout (each workspace crate gets one per profile
and per check/build mode), and a live one that idles past the cutoff costs
one non-incremental compile the next time that crate changes — cheap.
"""

from __future__ import annotations

import argparse
import filecmp
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HASH = re.compile(r"-([0-9a-f]{16})(?:[.-]|$)")

# The commands whose artifacts are worth keeping warm: the root workspace's
# check suite (clippy, tests) and the desktop app's. Anything these do not
# produce is considered stale.
WARM = [
    (ROOT, ["cargo", "clippy", "--workspace", "--all-targets"]),
    (ROOT, ["cargo", "test", "--workspace", "--no-run"]),
    (ROOT, ["cargo", "build", "--workspace"]),
    (ROOT / "app" / "src-tauri", ["cargo", "clippy", "--all-targets"]),
    (ROOT / "app" / "src-tauri", ["cargo", "build"]),
]


def target_dir() -> Path:
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        return Path(env).resolve()
    return ROOT / "target"


def tree_size(path: Path) -> int:
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    total = 0
    for dirpath, _, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(dirpath, f)).st_size
            except OSError:
                pass
    return total


def newest_mtime(path: Path) -> float:
    newest = path.stat().st_mtime
    for dirpath, dirs, files in os.walk(path):
        for name in dirs + files:
            try:
                newest = max(newest, os.lstat(os.path.join(dirpath, name)).st_mtime)
            except OSError:
                pass
    return newest


def fmt(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if abs(n) < 1024 or unit == "GB":
            return f"{n:.1f} {unit}" if unit != "B" else f"{n} B"
        n /= 1024
    return f"{n:.1f} GB"


def live_paths() -> tuple[set[Path], set[str]]:
    """Every artifact path the warm commands report, plus the unit hashes of
    uplifted binaries, or exit if a command fails."""
    paths: set[Path] = set()
    uplifted: set[str] = set()
    for cwd, cmd in WARM:
        full = cmd + ["--message-format=json"]
        print(f"  warm: {' '.join(cmd)}  (in {cwd.relative_to(ROOT) or '.'})")
        proc = subprocess.run(full, cwd=cwd, capture_output=True, text=True)
        if proc.returncode != 0:
            sys.stderr.write(proc.stderr)
            sys.stderr.write(
                f"\ngc-target: `{' '.join(cmd)}` failed; not pruning on a partial "
                "artifact list. Fix the build first.\n"
            )
            sys.exit(proc.returncode)
        reported: set[Path] = set()
        for line in proc.stdout.splitlines():
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            reason = msg.get("reason")
            if reason == "compiler-artifact":
                reported.update(Path(p) for p in msg.get("filenames", []))
                if msg.get("executable"):
                    reported.add(Path(msg["executable"]))
            elif reason == "build-script-executed" and msg.get("out_dir"):
                reported.add(Path(msg["out_dir"]))
        paths |= reported
        # An uplifted binary (`target/debug/oxen-harness`) is a byte-for-byte
        # copy of `deps/<crate>-<hash>`, and the JSON only names the copy. Two
        # profiles can uplift to the same path (`cargo test` links the
        # workspace bins under the test profile, `cargo build` under dev), so
        # match right now, before the next command overwrites the copy.
        for p in reported:
            if hash_of(p.name) is None and p.is_file() and (p.parent / "deps").is_dir():
                uplifted.update(matching_copies(p, p.parent / "deps"))
    return paths, uplifted


def matching_copies(uplift: Path, deps: Path) -> set[str]:
    stem = uplift.name.replace("-", "_")
    size = uplift.stat().st_size
    found: set[str] = set()
    for cand in deps.glob(f"{stem}-*"):
        if cand.suffix or not cand.is_file() or cand.stat().st_size != size:
            continue
        if filecmp.cmp(cand, uplift, shallow=False):
            h = hash_of(cand.name)
            if h:
                found.add(h)
    return found


def hash_of(name: str) -> str | None:
    m = HASH.search(name)
    return m.group(1) if m else None


def live_hashes(paths: set[Path], uplifted: set[str], profile: Path) -> set[str]:
    hashes = set(uplifted)
    for p in paths:
        h = hash_of(p.name) or (hash_of(p.parent.name) if p.parent.parent == profile / "build" else None)
        if h:
            hashes.add(h)
    return hashes


# Unhashed outputs (uplifted binaries, staticlib/cdylib archives, their .d
# files) are overwritten in place while a target keeps producing them, and
# left behind forever once it stops.
UNHASHED_SUFFIXES = {".a", ".rlib", ".dylib", ".so", ".dll", ".lib", ".exe", ".d", ""}


def stale_unhashed(entry: Path, profile: Path, paths: set[Path]) -> bool:
    if not entry.is_file() or entry.name.startswith("."):
        return False
    if entry.suffix not in UNHASHED_SUFFIXES:
        return False
    if entry.suffix == "":
        if not os.access(entry, os.X_OK):
            return False
        return entry not in paths
    if entry.suffix == ".d":
        return entry.with_suffix("") not in paths and not any(
            p.parent == entry.parent and p.stem == entry.stem for p in paths
        )
    return entry not in paths


def prune_profile(profile: Path, live: set[str], paths: set[Path], dry_run: bool) -> tuple[int, int]:
    """Remove hashed entries in deps/, build/ and .fingerprint/ not in `live`,
    and unhashed artifacts in deps/ and the profile root no target produces."""
    freed = 0
    count = 0
    for sub in ("deps", "build", ".fingerprint", "."):
        d = profile / sub
        if not d.is_dir():
            continue
        for entry in d.iterdir():
            h = hash_of(entry.name)
            if sub == ".":
                if h is not None or not stale_unhashed(entry, profile, paths):
                    continue
            elif h is None:
                if sub != "deps" or not stale_unhashed(entry, profile, paths):
                    continue
            elif h in live:
                continue
            size = tree_size(entry)
            freed += size
            count += 1
            if dry_run:
                print(f"    would remove {entry.relative_to(profile)}  ({fmt(size)})")
                continue
            if entry.is_dir() and not entry.is_symlink():
                shutil.rmtree(entry, ignore_errors=True)
            else:
                entry.unlink(missing_ok=True)
    return freed, count


def prune_by_age(entries: list[Path], cutoff: float, dry_run: bool, label: str) -> tuple[int, int]:
    freed = 0
    count = 0
    for entry in entries:
        if not entry.exists():
            continue
        if newest_mtime(entry) >= cutoff:
            continue
        size = tree_size(entry)
        freed += size
        count += 1
        if dry_run:
            print(f"    would remove {label} {entry.name}  ({fmt(size)})")
            continue
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry, ignore_errors=True)
        else:
            entry.unlink(missing_ok=True)
    return freed, count


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dry-run", action="store_true", help="report without deleting")
    ap.add_argument("--days", type=float, default=3, help="age after which incremental caches and other profiles are stale (default 3)")
    args = ap.parse_args()

    target = target_dir()
    if not target.is_dir():
        print(f"gc-target: no target directory at {target}; nothing to do")
        return 0
    before = tree_size(target)
    print(f"gc-target: {target} is {fmt(before)}")

    debug = target / "debug"
    paths, uplifted = live_paths()
    live = live_hashes(paths, uplifted, debug)
    print(f"  live: {len(paths)} artifact paths, {len(live)} unit hashes")

    freed = 0
    removed = 0
    if debug.is_dir():
        f, n = prune_profile(debug, live, paths, args.dry_run)
        freed += f
        removed += n
        print(f"  stale units in debug/: {n}  ({fmt(f)})")

        cutoff = time.time() - args.days * 86400
        inc = debug / "incremental"
        if inc.is_dir():
            f, n = prune_by_age(sorted(inc.iterdir()), cutoff, args.dry_run, "incremental")
            freed += f
            removed += n
            print(f"  incremental caches idle > {args.days:g}d: {n}  ({fmt(f)})")

    keep = {"debug", "tmp", "CACHEDIR.TAG", ".rustc_info.json"}
    others = [p for p in sorted(target.iterdir()) if p.name not in keep]
    if others:
        cutoff = time.time() - args.days * 86400
        f, n = prune_by_age(others, cutoff, args.dry_run, "profile dir")
        freed += f
        removed += n
        print(f"  other profile dirs idle > {args.days:g}d: {n}  ({fmt(f)})")

    verb = "would free" if args.dry_run else "freed"
    after = before - freed
    print(f"gc-target: {verb} {fmt(freed)} across {removed} entries; target is now ~{fmt(after)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
