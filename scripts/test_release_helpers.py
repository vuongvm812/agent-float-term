"""Release-only fixtures; no tags, network calls, real builds, or publication."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from release_source import check_remote, resolve, validate_tag


SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("formula", SCRIPTS / "generate-homebrew-formula.py")
FORMULA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FORMULA)
TAG = "v0.1.0"


class TagTests(unittest.TestCase):
    def test_semver(self):
        for tag in (TAG, "v10.20.30", "v0.0.0", "v1.2.3-rc.1", "v1.2.3-0.a-1", "v1.2.3--candidate"):
            with self.subTest(tag=tag):
                self.assertEqual(validate_tag(tag), tag[1:])
        for tag in ("1.2.3", "v01.2.3", "v1.02.3", "v1.2.03", "v1.2.3-01",
                    "v1.2.3-rc.01", "v1.2.3-rc..1", "v1.2.3-", "v1.2.3+build",
                    "v1.2.3\n", "v1.2.3/evil", "v1.2.3';exit", "v1.2.3-rc_1"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag)
        with self.assertRaises(ValueError):
            validate_tag("v1.2.3-rc.1", stable=True)

    def test_resolves_tagged_manifest_not_worktree(self):
        manifest = '[package]\nname = "agent-float-term"\nversion = "0.1.0"\n[dependencies]\n'
        with patch("release_source.subprocess.run") as run, patch(
            "release_source.subprocess.check_output", side_effect=["a" * 40, "b" * 40, manifest]
        ) as output:
            self.assertEqual(resolve(TAG), {"commit": "b" * 40, "tag_object": "a" * 40, "version": "0.1.0"})
            run.assert_called_once_with(["git", "check-ref-format", "refs/tags/v0.1.0"], check=True)
            self.assertEqual(output.call_args.args[0], ["git", "show", "b" * 40 + ":Cargo.toml"])

    def test_rejects_tagged_version_mismatch_or_ambiguous_metadata(self):
        for manifest in (
            '[package]\nname = "agent-float-term"\nversion = "0.2.0"\n',
            '[package]\nname = "other"\nversion = "0.1.0"\n',
            '[package]\nname = "agent-float-term"\nversion.workspace = true\n',
            '[workspace]\nmembers = []\n',
            '[package]\nname = "agent-float-term"\nversion = "0.1.0"\nversion = "0.1.0"\n',
        ):
            with self.subTest(manifest=manifest), patch("release_source.subprocess.run"), patch(
                "release_source.subprocess.check_output", side_effect=["a" * 40, "b" * 40, manifest]
            ), self.assertRaises(ValueError):
                resolve(TAG)

    def test_missing_tag_refuses(self):
        with patch("release_source.subprocess.run"), patch(
            "release_source.subprocess.check_output", side_effect=subprocess.CalledProcessError(128, "git")
        ), self.assertRaises(subprocess.CalledProcessError):
            resolve(TAG)

    def test_remote_lightweight_and_nested_annotated_tags(self):
        for objects, tag_object in (
            ([("commit", "b" * 40)], "b" * 40),
            ([("tag", "a" * 40), ("tag", "c" * 40), ("commit", "b" * 40)], "a" * 40),
        ):
            replies = [json.dumps({"object": {"type": kind, "sha": sha}}) for kind, sha in objects]
            with patch("release_source.subprocess.check_output", side_effect=replies) as output:
                check_remote(TAG, "b" * 40, tag_object, "owner/repo")
                self.assertEqual(output.call_count, len(objects))

    def test_remote_moved_reannotated_or_noncommit_tag_refuses(self):
        for objects in (
            [("commit", "c" * 40)],
            [("tag", "c" * 40), ("commit", "b" * 40)],
            [("tag", "a" * 40), ("commit", "c" * 40)],
            [("tag", "a" * 40), ("tree", "b" * 40)],
        ):
            replies = [json.dumps({"object": {"type": kind, "sha": sha}}) for kind, sha in objects]
            with self.subTest(objects=objects), patch(
                "release_source.subprocess.check_output", side_effect=replies
            ), self.assertRaises(ValueError):
                check_remote(TAG, "b" * 40, "a" * 40, "owner/repo")


class FormulaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        tools = self.root / "tools"
        tools.mkdir()
        # Execute the real packaging script with fixture build tools, so archive layout
        # changes in package-release.sh break these tests instead of silently drifting.
        (tools / "rustc").write_text('#!/bin/sh\nprintf "host: %s\\n" "$FIXTURE_TARGET"\n')
        (tools / "cargo").write_text(
            '#!/bin/sh\ncase "$2" in\n'
            'metadata) printf \'{"packages":[{"name":"agent-float-term","version":"%s"}]}\\n\' "${FIXTURE_VERSION:-0.1.0}" ;;\n'
            'build) exit 0 ;;\n*) exit 1 ;;\nesac\n'
        )
        for tool in tools.iterdir():
            tool.chmod(0o755)
        (self.root / "LICENSE").write_text("fixture license\n")
        (self.root / "README.md").write_text("fixture readme\n")
        for target in FORMULA.TARGETS:
            binary = self.root / "target" / target / "release" / "agent-float-term"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"fixture executable, never published\n")
            binary.chmod(0o755)
            subprocess.run(
                ["bash", str(SCRIPTS / "package-release.sh"), TAG, target], cwd=self.root,
                env={**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                     "FIXTURE_TARGET": target, "COPYFILE_DISABLE": "1"},
                check=True, capture_output=True, text=True,
            )
        self.sums = self.root / "dist" / "SHA256SUMS"
        self.write_sums()

    def write_sums(self):
        self.sums.write_text("".join(
            f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n"
            for archive in sorted(self.sums.parent.glob("*.tar.gz"))
        ))

    def test_real_packager_layout_checksums_urls_and_ruby_syntax(self):
        formula = FORMULA.generate(TAG, self.sums)
        self.assertNotIn("@VERSION@", formula)
        for target in FORMULA.TARGETS:
            name = f"agent-float-term-{TAG}-{target}.tar.gz"
            self.assertIn(f"releases/download/{TAG}/{name}", formula)
            self.assertIn(hashlib.sha256((self.sums.parent / name).read_bytes()).hexdigest(), formula)
        subprocess.run(["ruby", "-c"], input=formula, text=True, check=True, capture_output=True)

        # SemVer permits a prerelease identifier starting with a hyphen. The tag
        # resolver and native packager must agree before preparing a candidate.
        tag = "v0.1.0--candidate"
        target = FORMULA.TARGETS[0]
        validate_tag(tag)
        subprocess.run(
            ["bash", str(SCRIPTS / "package-release.sh"), tag, target], cwd=self.root,
            env={**os.environ, "PATH": str(self.root / "tools") + os.pathsep + os.environ["PATH"],
                 "FIXTURE_TARGET": target, "FIXTURE_VERSION": tag[1:], "COPYFILE_DISABLE": "1"},
            check=True, capture_output=True, text=True,
        )

    def test_invalid_checksum_manifests(self):
        original = self.sums.read_text()
        lines = original.splitlines()
        variants = (
            "", "\n".join(lines[:2]) + "\n", original + lines[0] + "\n",
            original + "f" * 64 + "  extra.tar.gz\n",
            original.replace(lines[0][:64], "g" * 64),
            original.replace(lines[0][:64], "a" * 63),
            original.replace("  agent-", "  ../agent-", 1),
            original.replace("  agent-", "  ./agent-", 1),
            original.replace("  agent-", "  sub\\agent-", 1),
            original.replace("  agent-", " agent-", 1),
            original + "\n", original.replace("v0.1.0", "v0.2.0", 1),
        )
        for value in variants:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.sums.write_text(value)
                FORMULA.generate(TAG, self.sums)

    def test_binary_marker_and_uppercase_checksums(self):
        self.sums.write_text("\n".join(
            line[:64].upper() + " *" + line[66:] for line in self.sums.read_text().splitlines()
        ) + "\n")
        FORMULA.generate(TAG, self.sums)

    def test_corrupted_archive_or_missing_archive_refuses(self):
        archive = next(self.sums.parent.glob("*.tar.gz"))
        archive.write_bytes(archive.read_bytes() + b"changed")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            FORMULA.generate(TAG, self.sums)
        archive.unlink()
        with self.assertRaises(FileNotFoundError):
            FORMULA.generate(TAG, self.sums)

    def test_invalid_archive_layout_or_members_refuses(self):
        archive = next(self.sums.parent.glob("*.tar.gz"))
        with tarfile.open(archive) as tar:
            original = [(member, tar.extractfile(member).read() if member.isfile() else None)
                        for member in tar.getmembers()]
        for mutation in ("extra", "duplicate", "symlink", "nonexecutable", "setuid", "empty"):
            with self.subTest(mutation=mutation):
                with tarfile.open(archive, "w:gz") as tar:
                    for member, data in original:
                        info = tarfile.TarInfo(member.name)
                        info.type, info.mode, info.size = member.type, member.mode, member.size
                        if member.name.endswith("/agent-float-term"):
                            if mutation == "symlink":
                                info.type, info.linkname, info.size, data = tarfile.SYMTYPE, "/bin/sh", 0, None
                            elif mutation == "nonexecutable":
                                info.mode = 0o644
                            elif mutation == "setuid":
                                info.mode = 0o4755
                            elif mutation == "empty":
                                info.size, data = 0, b""
                        tar.addfile(info, io.BytesIO(data) if data is not None else None)
                    if mutation in ("extra", "duplicate"):
                        tar.addfile(tarfile.TarInfo("../unexpected" if mutation == "extra" else original[0][0].name))
                self.write_sums()
                with self.assertRaises(ValueError):
                    FORMULA.generate(TAG, self.sums)

    def test_prerelease_and_wrong_tag_refuse(self):
        for tag in ("v0.1.0-rc.1", "v0.2.0", "v00.1.0"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                FORMULA.generate(tag, self.sums)

    def test_cli_stdout_new_output_and_no_overwrites(self):
        command = [os.sys.executable, "-B", str(SCRIPTS / "generate-homebrew-formula.py"), TAG, str(self.sums)]
        expected = FORMULA.generate(TAG, self.sums)
        result = subprocess.run(command, check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout, expected)
        output = self.root / "formula.rb"
        result = subprocess.run(command + ["--output", str(output)], check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout, "")
        self.assertEqual(output.read_text(), expected)
        for path in (output, self.root / "symlink.rb", self.root / "dangling.rb"):
            if path != output:
                path.symlink_to(output if path.name == "symlink.rb" else self.root / "missing")
            result = subprocess.run(command + ["--output", str(path)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_text(), expected)
        self.sums.write_text("invalid\n")
        invalid_output = self.root / "invalid.rb"
        result = subprocess.run(command + ["--output", str(invalid_output)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(invalid_output.exists())


class PackageTests(unittest.TestCase):
    def test_locked_package_list_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo = root / "cargo"
            cargo.write_text(
                '#!/bin/sh\n[ "$1" = +1.84.1 ] && [ "$2" = package ] && [ "$3" = --locked ] || exit 9\n'
                'if [ "$#" = 4 ] && [ "$4" = --list ]; then\n'
                '  printf \'%s\\n\' "$FIXTURE_PACKAGE_LIST"\n'
                'elif [ "$#" != 3 ]; then\n  exit 9\nfi\n'
            )
            cargo.chmod(0o755)
            required = "Cargo.toml\nCargo.lock\nsrc/main.rs\nsrc/lib.rs\nREADME.md\nLICENSE"
            for listing, succeeds in (
                (required, True), (required.replace("Cargo.lock\n", ""), False),
                *((required + "\n" + name, False) for name in (
                    "dist/archive", "target/debug/agent-float-term", ".codegraph/index",
                    ".github/workflows/ci.yml", "scripts/__pycache__/module.pyc", "local.tar.gz",
                )),
            ):
                with self.subTest(listing=listing):
                    result = subprocess.run(
                        ["bash", str(SCRIPTS / "verify-cargo-package.sh")], cwd=root,
                        env={**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                             "FIXTURE_PACKAGE_LIST": listing}, capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
