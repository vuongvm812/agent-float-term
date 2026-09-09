"""Exercise installation ordering with fake build tools, never a live install."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class InstallLocalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aft-install-local-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "source with spaces"
        for path in ("scripts", "target/release", "bin"):
            (self.repo / path).mkdir(parents=True)
        shutil.copyfile(Path(__file__).with_name("install-local.sh"),
                        self.repo / "scripts/install-local.sh")
        self.log = self.root / "calls"
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = {
            "PATH": str(self.repo / "bin") + os.pathsep + os.defpath,
            "HOME": str(self.home),
            "XDG_DATA_HOME": str(self.home / "custom data"),
            "TEST_LOG": str(self.log),
        }
        self.script("bin/cargo", 'printf "cargo:%s\\n" "$*" >> "$TEST_LOG"\nexit "${FAIL_CARGO:-0}"')
        self.script("target/release/agent-float-term", '''
printf 'app:%s\n' "$*" >> "$TEST_LOG"
if [ "$*" = install ]; then exit "${FAIL_PREVIEW:-0}"; fi
exit "${FAIL_APPLY:-0}"
''')
        self.script("scripts/build-mouse-tmux.sh", '''
printf 'tmux-build\n' >> "$TEST_LOG"
[ "${FAIL_TMUX:-0}" = 0 ] || exit 1
mkdir "$1"
printf '%s/tmux\n' "$1"
''')

    def script(self, name, body):
        path = self.repo / name
        path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
        path.chmod(0o700)

    def run_install(self, **env):
        return subprocess.run(
            ["bash", str(self.repo / "scripts/install-local.sh")],
            cwd=self.root, env={**self.env, **env}, capture_output=True,
            text=True, timeout=10,
        )

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_dry_run_has_no_build_or_install_side_effects(self):
        result = self.run_install(DRY_RUN="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Plan only", result.stdout)
        self.assertEqual(self.calls(), [])
        self.assertEqual(list(self.home.iterdir()), [])

    def test_install_orders_apply_after_build_and_preserves_previous_builds(self):
        for _ in range(2):
            result = self.run_install()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("AFT_TMUX_BINARY=", result.stdout)
            self.assertIn("Existing tmux servers and sessions were not changed", result.stdout)
            launch = next(line for line in result.stdout.splitlines()
                          if line.startswith("AFT_TMUX_BINARY="))
            syntax = subprocess.run(["bash", "-n"], input=launch, text=True,
                                    capture_output=True, timeout=10)
            self.assertEqual(syntax.returncode, 0, syntax.stderr)
        self.assertEqual(self.calls(), [
            "cargo:+1.84.1 build --locked --release", "app:install",
            "tmux-build", "app:install --yes",
        ] * 2)
        builds = list((Path(self.env["XDG_DATA_HOME"]) / "agent-float-term/compat").iterdir())
        self.assertEqual(len(builds), 2)
        for build in builds:
            self.assertEqual(build.stat().st_mode & 0o777, 0o700)
            self.assertTrue((build / "build").is_dir())
            self.assertEqual((build / ".make-install-owned").read_text(),
                             "agent-float-term make install tmux v1\n")

    def test_failures_stop_before_later_mutations(self):
        for failure, expected_count in (("FAIL_CARGO", 1), ("FAIL_PREVIEW", 2),
                                        ("FAIL_TMUX", 3), ("FAIL_APPLY", 4)):
            with self.subTest(failure=failure):
                self.log.unlink(missing_ok=True)
                result = self.run_install(**{failure: "1"})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.calls()), expected_count)
                self.assertNotIn("Installation complete", result.stdout)

    def test_invalid_options_fail_without_building(self):
        for env in ({"DRY_RUN": "yes"}, {"XDG_DATA_HOME": "relative"}):
            with self.subTest(env=env):
                self.assertNotEqual(self.run_install(**env).returncode, 0)
                self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
