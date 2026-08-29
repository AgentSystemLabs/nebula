#!/usr/bin/env python3
"""Drop stale cargo build artifacts from target/ (the TARGET PRUNE behind `make prune`).

Every distinct build configuration gets its own hash in target/<profile>/: a
version bump at release re-hashes every workspace crate, and `cargo build`,
`cargo test`, `cargo check` and `cargo clippy` each resolve features a little
differently and hash separately again. On macOS each hash also keeps every
object file beside its binary (`split-debuginfo=unpacked`, the dev default),
~200MB per build of nebula_tui alone — and nothing ever removes the old ones.
Ten days of SESSIONS grew target/ to 41GB and filled the disk.

This keeps the newest KEEP hashes of every crate (by the newest mtime among
that hash's files) and deletes the rest, in deps/, incremental/, .fingerprint/
and build/, for every profile dir that exists. A hash that is still in use but
got evicted only costs cargo one recompile of that crate; a fingerprint whose
outputs are gone is rebuilt, never trusted. It holds cargo's own build lock
(target/<profile>/.cargo-lock) while it works, so it waits for a running build
instead of deleting under it, and no build starts mid-prune.
"""
import argparse
import fcntl
import os
import re
import shutil
import sys

HEX16 = r"[0-9a-f]{16}"
# subdir -> the hash shape in its entry names (`<stem>-<hash>` then `.`, `-` or end)
LAYOUT = {
    "deps": HEX16,
    ".fingerprint": HEX16,
    "build": HEX16,
    "incremental": r"[0-9a-z]{13}",
}


def newest_mtime(path):
    """Newest mtime under a dir entry (a file's own mtime, a dir's deepest one)."""
    try:
        newest = os.lstat(path).st_mtime
    except OSError:
        return 0.0
    if not os.path.isdir(path) or os.path.islink(path):
        return newest
    for root, _dirs, files in os.walk(path):
        for name in files:
            try:
                newest = max(newest, os.lstat(os.path.join(root, name)).st_mtime)
            except OSError:
                pass
    return newest


def stale_entries(subdir, hash_pat, keep):
    """Paths of every entry belonging to a hash that is not among a stem's newest `keep`."""
    rx = re.compile(rf"^(.+?)-({hash_pat})(?:[.-]|$)")
    groups = {}  # (stem, hash) -> [paths, newest mtime]
    with os.scandir(subdir) as it:
        for entry in it:
            m = rx.match(entry.name)
            if not m:
                continue
            group = groups.setdefault((m.group(1), m.group(2)), [[], 0.0])
            group[0].append(entry.path)
            group[1] = max(group[1], newest_mtime(entry.path))
    by_stem = {}
    for (stem, h), (paths, mtime) in groups.items():
        by_stem.setdefault(stem, []).append((mtime, h, paths))
    stale, hashes = [], 0
    for stem, builds in by_stem.items():
        builds.sort(reverse=True)
        for _mtime, _h, paths in builds[keep:]:
            stale.extend(paths)
            hashes += 1
    return stale, hashes, len(by_stem)


def remove(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path, ignore_errors=True)
    else:
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass


def free_bytes(path):
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize


def prune_profile(profile_dir, keep, dry_run):
    lock_path = os.path.join(profile_dir, ".cargo-lock")
    with open(lock_path, "a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f"  {profile_dir}: waiting for a running cargo build to finish…", flush=True)
            fcntl.flock(lock, fcntl.LOCK_EX)
        before = free_bytes(profile_dir)
        for name, hash_pat in LAYOUT.items():
            subdir = os.path.join(profile_dir, name)
            if not os.path.isdir(subdir):
                continue
            stale, hashes, stems = stale_entries(subdir, hash_pat, keep)
            verb = "would drop" if dry_run else "dropped"
            print(f"  {os.path.relpath(subdir)}: {verb} {hashes} stale builds "
                  f"({len(stale)} entries) across {stems} crates", flush=True)
            if not dry_run:
                for path in stale:
                    remove(path)
        if not dry_run:
            freed = free_bytes(profile_dir) - before
            print(f"  {os.path.relpath(profile_dir)}: freed {freed / 1e9:.1f} GB", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--target", default="target", help="the cargo target dir (default: target)")
    ap.add_argument("--keep", type=int, default=3,
                    help="how many newest builds of each crate to keep (default: 3)")
    ap.add_argument("--dry-run", action="store_true", help="report only, delete nothing")
    args = ap.parse_args()
    if args.keep < 1:
        sys.exit("--keep must be at least 1")
    profiles = [os.path.join(args.target, p) for p in ("debug", "release")]
    profiles = [p for p in profiles if os.path.isdir(p)]
    if not profiles:
        print(f"nothing to prune: no profile dirs under {args.target}/")
        return
    for profile_dir in profiles:
        prune_profile(profile_dir, args.keep, args.dry_run)


if __name__ == "__main__":
    main()
