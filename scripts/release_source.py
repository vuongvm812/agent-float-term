"""Resolve and recheck existing release tags without creating or moving refs."""

import argparse
import json
import re
import subprocess
from urllib.parse import quote


def validate_tag(tag, stable=False):
    number = r"(?:0|[1-9][0-9]*)"
    if not re.fullmatch(rf"v{number}\.{number}\.{number}(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", tag):
        raise ValueError("expected vMAJOR.MINOR.PATCH[-PRERELEASE], without build metadata")
    if "-" in tag:
        if stable:
            raise ValueError("the Homebrew formula requires a stable release tag")
        for part in tag.split("-", 1)[1].split("."):
            if part.isdigit() and len(part) > 1 and part.startswith("0"):
                raise ValueError("numeric prerelease identifiers must not have leading zeroes")
    return tag[1:]


def resolve(tag):
    version = validate_tag(tag)
    ref = f"refs/tags/{tag}"
    subprocess.run(["git", "check-ref-format", ref], check=True)
    tag_object = subprocess.check_output(
        ["git", "rev-parse", "--verify", ref + "^{object}"], text=True
    ).strip()
    commit = subprocess.check_output(
        ["git", "rev-parse", "--verify", ref + "^{commit}"], text=True
    ).strip()
    manifest = subprocess.check_output(["git", "show", f"{commit}:Cargo.toml"], text=True)
    package = re.search(r"(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)", manifest)
    if package is None:
        raise ValueError("missing [package] metadata in tagged Cargo.toml")
    # Refuse unfamiliar/inherited TOML metadata rather than guessing a release version.
    names = re.findall(r'^name\s*=\s*"([^"\n]+)"\s*$', package[1], re.MULTILINE)
    versions = re.findall(r'^version\s*=\s*"([^"\n]+)"\s*$', package[1], re.MULTILINE)
    if names != ["agent-float-term"] or versions != [version]:
        raise ValueError("tag does not match the tagged agent-float-term package version")
    return {"commit": commit, "tag_object": tag_object, "version": version}


def check_remote(tag, commit, tag_object, repository):
    validate_tag(tag)
    if not all(re.fullmatch(r"[0-9a-f]{40}", value) for value in (commit, tag_object)):
        raise ValueError("expected full resolved Git object IDs")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("expected OWNER/REPO")

    def api(path):
        return json.loads(subprocess.check_output(["gh", "api", path], text=True))["object"]

    obj = api(f"repos/{repository}/git/ref/tags/{quote(tag, safe='')}")
    if obj["sha"] != tag_object:
        raise ValueError("tag object moved after resolution; refusing publication")
    for _ in range(16):
        if obj["type"] != "tag":
            break
        obj = api(f"repos/{repository}/git/tags/{obj['sha']}")
    if obj["type"] != "commit" or obj["sha"] != commit:
        raise ValueError("tag no longer resolves to the verified source commit")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("resolve").add_argument("tag")
    check = commands.add_parser("check")
    for argument in ("tag", "commit", "tag_object", "repository"):
        check.add_argument(argument)
    args = parser.parse_args()
    try:
        if args.command == "resolve":
            for key, value in resolve(args.tag).items():
                print(f"{key}={value}")
        else:
            check_remote(args.tag, args.commit, args.tag_object, args.repository)
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"release source validation failed: {error}\n")


if __name__ == "__main__":
    main()
