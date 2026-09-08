#!/usr/bin/env bash
set -euo pipefail

# Inspect Cargo's actual upload list before building the unpacked crate with its lockfile.
cargo +1.84.1 package --locked --list | python3 -c '
import pathlib, sys
files = set(sys.stdin.read().splitlines())
required = {"Cargo.toml", "Cargo.lock", "src/main.rs", "src/lib.rs", "README.md", "LICENSE"}
missing = required - files
forbidden = {"dist", "target", ".codegraph", ".github", "__pycache__", ".git"}
bad = sorted(name for name in files if forbidden.intersection(pathlib.PurePosixPath(name).parts)
             or name.endswith((".sock", ".pyc", ".tar.gz")))
print("Crate upload contents:\n" + "\n".join(sorted(files)))
if missing or bad:
    sys.exit(f"Invalid crate contents: missing={sorted(missing)}, local artifacts={bad}")
'
cargo +1.84.1 package --locked
