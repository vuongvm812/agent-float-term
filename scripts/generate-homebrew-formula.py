"""Render a stable binary formula from SHA256SUMS and the adjacent release archives.

Run from the exact resolved release source checkout. No network access, source build,
tap update, or default output file: stdout unless --output names a new file.
"""

import argparse
import hashlib
from pathlib import Path
import re
import tarfile

from release_source import validate_tag


TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu")


def generate(tag, sums_path):
    version = validate_tag(tag, stable=True)
    expected = {f"agent-float-term-{tag}-{target}.tar.gz": target for target in TARGETS}
    sums = {}
    for line in sums_path.read_text(encoding="ascii").splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]([^\s/\\]+)", line)
        if not match:
            raise ValueError("invalid SHA256SUMS line (expected sha256sum format)")
        digest, name = match.groups()
        if name not in expected or name in sums:
            raise ValueError(f"unexpected or duplicate checksum name: {name}")
        sums[name] = digest.lower()
    if set(sums) != set(expected):
        raise ValueError("SHA256SUMS must contain exactly the three named release archives")

    replacements = {"VERSION": version, "TAG": tag}
    for name, target in expected.items():
        archive = sums_path.parent / name
        digest = hashlib.sha256()
        with archive.open("rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        if digest.hexdigest() != sums[name]:
            raise ValueError(f"archive checksum mismatch: {name}")
        # Homebrew strips this single enclosing directory before `bin.install`.
        root = name.removesuffix(".tar.gz")
        with tarfile.open(archive, "r:gz") as tar:
            members = tar.getmembers()
            names = [member.name.rstrip("/") for member in members]
            if len(names) != 4 or set(names) != {
                root, f"{root}/agent-float-term", f"{root}/LICENSE", f"{root}/README.md"
            }:
                raise ValueError(f"unexpected release archive layout: {name}: {names}")
            for member in members:
                if member.name.rstrip("/") == root:
                    if not member.isdir():
                        raise ValueError(f"archive root is not a directory: {name}")
                elif not member.isfile() or member.mode & 0o6000:
                    raise ValueError(f"unsafe archive member: {member.name}")
                elif member.name.endswith("/agent-float-term") and (
                    not member.mode & 0o111 or member.size == 0
                ):
                    raise ValueError(f"archive binary is empty or not executable: {name}")
        replacements[target.upper().replace("-", "_") + "_SHA256"] = sums[name]

    template = Path(__file__).with_name("agent-float-term.rb.in").read_text(encoding="ascii")
    for key, value in replacements.items():
        template = template.replace(f"@{key}@", value)
    if re.search(r"@[A-Z0-9_]+@", template):
        raise ValueError("unresolved formula template token")
    return template


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("sha256sums", type=Path)
    parser.add_argument("--output", type=Path, help="create a new file; never overwrite an existing path")
    args = parser.parse_args()
    try:
        formula = generate(args.tag, args.sha256sums)
        if args.output:
            with args.output.open("x", encoding="ascii") as output:
                output.write(formula)
        else:
            print(formula, end="")
    except (ValueError, OSError, UnicodeError, tarfile.TarError) as error:
        parser.exit(1, f"formula generation failed: {error}\n")


if __name__ == "__main__":
    main()
