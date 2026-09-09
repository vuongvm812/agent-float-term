#!/usr/bin/env python3
"""Remove managed installation and idle Make-owned tmux builds, never servers."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
MARKER = ".make-install-owned"
OWNER = b"agent-float-term make install tmux v1\n"
TMUX_SHA256 = "7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf"


class UninstallError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise UninstallError(message)


def directory(path, private=False):
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(),
            f"Not an owned directory (symlinks are refused): {path}")
    require(not info.st_mode & (0o077 if private else 0o022), f"Unsafe directory permissions: {path}")
    return info.st_dev, info.st_ino


def parents(path):
    for parent in reversed((path, *path.parents)):
        if not os.path.lexists(parent):
            continue
        info = parent.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0, os.getuid()),
                f"Unsafe directory ancestry: {parent}")
        require(not info.st_mode & 0o022 or info.st_uid == 0 and info.st_mode & stat.S_ISVTX,
                f"Writable directory ancestry: {parent}")


def contents(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
                and not info.st_mode & 0o022 and info.st_size <= limit,
                f"Unsafe or oversized ownership record: {path}")
        data = stream.read(limit + 1)
        require(len(data) <= limit, f"Ownership record changed: {path}")
        return data


def owned_build(path):
    identity = directory(path, private=True)
    entries = {entry.name for entry in path.iterdir()}
    if MARKER in entries:
        require(entries <= {MARKER, "build"} and contents(path / MARKER, 256) == OWNER,
                f"Changed Make build ownership record or unexpected files: {path}")
        if "build" in entries:
            directory(path / "build", private=True)
    else:
        # Earlier make install versions had no marker. Adopt only their exact
        # top-level layout and checksum-verified upstream archive, not a name alone.
        require(entries == {"build"}, f"Unrecognized Make build; inspect manually: {path}")
        build = path / "build"
        directory(build, private=True)
        require({entry.name for entry in build.iterdir()} ==
                {"build.log", "tmux-3.7c.tar.gz", "tmux-3.7c"},
                f"Unrecognized legacy build layout; inspect manually: {path}")
        directory(build / "tmux-3.7c")
        require(hashlib.sha256(contents(build / "tmux-3.7c.tar.gz", 16 * 1024 * 1024)).hexdigest()
                == TMUX_SHA256, f"Legacy build archive differs; inspect manually: {path}")
    def unreadable(error):
        raise error
    for parent, directories, files in os.walk(path, followlinks=False, onerror=unreadable):
        for name in directories + files:
            entry = Path(parent) / name
            info = entry.lstat()
            require(info.st_uid == os.getuid() and info.st_dev == identity[0]
                    and not entry.is_mount(), f"Foreign owner or mounted content in build: {entry}")
    return identity


def idle(path):
    tool = shutil.which("lsof")
    require(tool is not None, "lsof is required to check tmux builds for use; no files were removed")
    result = subprocess.run([tool, "-nP", "-t", "+D", str(path)],
                            capture_output=True, text=True, timeout=30)
    require(not result.stdout.strip(),
            f"Tmux build is in use; close its processes before uninstalling: {path}\n"
            f"Process IDs: {result.stdout.strip()}")
    require(result.returncode == 1 and not result.stderr.strip(),
            f"Cannot establish that tmux build is idle: {path}\n{result.stderr.strip()}")


def main():
    require(len(sys.argv) == 1 and os.environ.get("DRY_RUN", "") in ("", "0", "1"),
            "Usage: make uninstall [DRY_RUN=1]")
    dry = os.environ.get("DRY_RUN") == "1"
    home = Path(os.environ.get("HOME", ""))
    require(home.is_absolute(), "HOME must be absolute")
    home = home.resolve()
    data = Path(os.environ.get("XDG_DATA_HOME") or home / ".local/share")
    state = Path(os.environ.get("XDG_STATE_HOME") or home / ".local/state")
    require(data.is_absolute() and state.is_absolute(), "XDG directory roots must be absolute")
    data = data.resolve() / "agent-float-term"
    state = state.resolve() / "agent-float-term"
    compat = data / "compat"
    parents(compat)
    parents(state)

    builds = []
    if os.path.lexists(compat):
        directory(compat)
        for path in sorted(compat.iterdir()):
            if re.fullmatch(r"tmux-status-mouse\.[A-Za-z0-9]{8}", path.name):
                builds.append((path, owned_build(path)))
    if builds:
        require(shutil.rmtree.avoids_symlink_attacks, "Safe directory removal is unavailable on this platform")
    # Check every candidate before removing even the application. Never stop a
    # server to make this check pass, and do not bypass inconclusive lsof results.
    for path, _ in builds:
        idle(path)

    manifest = state / "install.json"
    app = None
    if os.path.lexists(manifest):
        record = json.loads(contents(manifest, 4 * 1024 * 1024))
        require(isinstance(record, dict) and record.get("external") is None
                and record.get("format") in (1, 2),
                "make uninstall is for managed source/archive installs, not Homebrew/Cargo registrations")
        built = ROOT / "target/release/agent-float-term"
        if built.is_file() and os.access(built, os.X_OK):
            app = built
        else:
            installed = home / ".local/bin/agent-float-term"
            current = record.get("current")
            if isinstance(current, str) and re.fullmatch(r"[0-9a-f]{64}", current):
                expected = data / "releases" / current / "agent-float-term"
                parents(expected.parent)
                if installed.is_file() and installed.resolve() == expected and os.access(installed, os.X_OK):
                    require(hashlib.sha256(contents(expected, 128 * 1024 * 1024)).hexdigest() == current,
                            "Installed uninstaller was modified; build a fresh application first")
                    app = installed
        require(app is not None, "No application uninstaller found; build the application first")
        subprocess.run([str(app), "uninstall"], check=True, timeout=120)
    else:
        print("No managed application manifest; application files will not be removed.")
    for path, _ in builds:
        print(f"Remove verified idle Make build: {path}")
    if dry:
        print("DRY_RUN: no files or bindings changed; no servers stopped.")
        return
    sys.stdout.flush()
    if app is not None:
        subprocess.run([str(app), "uninstall", "--yes"], check=True, timeout=120)
    for path, identity in builds:
        parents(compat)
        require(owned_build(path) == identity, f"Build directory changed; left untouched: {path}")
        idle(path)
        shutil.rmtree(path)
    print("Uninstall complete. Unrelated compatibility builds, repository build output, and tmux servers were preserved.")


if __name__ == "__main__":
    try:
        main()
    except (UninstallError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Uninstall stopped: {error}", file=sys.stderr)
        sys.exit(1)
