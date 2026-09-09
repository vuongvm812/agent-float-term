"""Uninstall only private fixtures; no live app or tmux server is touched."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location("uninstall_local", Path(__file__).with_name("uninstall-local.py"))
uninstall = importlib.util.module_from_spec(spec)
spec.loader.exec_module(uninstall)


class UninstallLocalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aft-uninstall-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.home = self.root / "home"
        self.home.mkdir(mode=0o700)
        self.data = self.home / "custom data/agent-float-term"
        self.compat = self.data / "compat"
        self.compat.mkdir(parents=True, mode=0o700)
        self.manifest = self.home / "state/agent-float-term/install.json"
        self.manifest.parent.mkdir(parents=True)
        self.manifest.write_text(json.dumps({"format": 2, "external": None}))
        self.repo = self.root / "repo"
        self.app = self.repo / "target/release/agent-float-term"
        self.app.parent.mkdir(parents=True)
        self.app.write_text("fixture; never executed")
        self.app.chmod(0o700)
        self.calls = []
        self.probes = []
        self.busy = False
        self.probe_error = ""
        self.apply_error = False
        self.on_apply = lambda: None

    def build(self, name="tmux-status-mouse.ABC12345", legacy=False):
        path = self.compat / name
        path.mkdir(mode=0o700)
        build = path / "build"
        build.mkdir(mode=0o700)
        if legacy:
            (build / "tmux-3.7c").mkdir()
            (build / "build.log").write_text("Verified fixture archive\n")
            (build / "tmux-3.7c.tar.gz").write_bytes(b"fixture archive")
        else:
            (path / uninstall.MARKER).write_bytes(uninstall.OWNER)
        return path

    def command(self, args, **kwargs):
        if args[0] == "/fixture/lsof":
            self.probes.append(args[-1])
            return subprocess.CompletedProcess(args, 0 if self.busy else 1,
                                               "123\n" if self.busy else "", self.probe_error)
        self.assertEqual(args[0], str(self.app))
        self.calls.append(args[1:])
        if args[-1] == "--yes":
            if self.apply_error:
                raise subprocess.CalledProcessError(1, args)
            self.manifest.unlink()
            self.on_apply()
        return subprocess.CompletedProcess(args, 0)

    def run_uninstall(self, dry=False, lsof=True):
        env = {"HOME": str(self.home), "XDG_DATA_HOME": str(self.data.parent),
               "XDG_STATE_HOME": str(self.home / "state"), "DRY_RUN": "1" if dry else "0"}
        with mock.patch.dict(os.environ, env), mock.patch.object(sys, "argv", ["uninstall-local.py"]), \
                mock.patch.object(uninstall, "ROOT", self.repo), \
                mock.patch.object(uninstall.shutil, "which", return_value="/fixture/lsof" if lsof else None), \
                mock.patch.object(uninstall.subprocess, "run", side_effect=self.command), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            uninstall.main()
        return output.getvalue()

    def test_removes_owned_builds_and_app_but_preserves_unrelated_files(self):
        owned = self.build()
        other = self.compat / "tmux-3.5a"
        other.mkdir()
        note = self.data / "notes.txt"
        note.write_text("keep")
        self.run_uninstall()
        self.assertFalse(owned.exists())
        self.assertFalse(self.manifest.exists())
        self.assertTrue(other.exists())
        self.assertEqual(note.read_text(), "keep")
        self.assertEqual(self.calls, [["uninstall"], ["uninstall", "--yes"]])
        self.assertEqual(len(self.probes), 2)
        self.run_uninstall()  # Already uninstalled is harmless.
        self.assertEqual(len(self.calls), 2)

    def test_dry_run_only_previews(self):
        owned = self.build()
        self.assertIn("DRY_RUN", self.run_uninstall(dry=True))
        self.assertTrue(owned.exists())
        self.assertTrue(self.manifest.exists())
        self.assertEqual(self.calls, [["uninstall"]])

    def test_busy_unknown_or_unavailable_probe_refuses_before_app_changes(self):
        owned = self.build()
        for busy, error, available in [(True, "", True), (False, "cannot inspect", True),
                                       (False, "", False)]:
            with self.subTest(busy=busy, error=error, available=available):
                self.busy, self.probe_error = busy, error
                with self.assertRaises(uninstall.UninstallError):
                    self.run_uninstall(lsof=available)
                self.assertTrue(owned.exists())
                self.assertTrue(self.manifest.exists())
                self.assertEqual(self.calls, [])

    def test_application_failure_preserves_builds(self):
        owned = self.build()
        self.apply_error = True
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_uninstall()
        self.assertTrue(owned.exists())

    def test_newly_busy_build_is_preserved_after_app_removal(self):
        owned = self.build()
        self.on_apply = lambda: setattr(self, "busy", True)
        with self.assertRaisesRegex(uninstall.UninstallError, "in use"):
            self.run_uninstall()
        self.assertTrue(owned.exists())

    def test_changed_marker_or_extra_files_are_not_removed(self):
        owned = self.build()
        marker = owned / uninstall.MARKER
        marker.write_text("not owned")
        with self.assertRaises(uninstall.UninstallError):
            self.run_uninstall()
        marker.write_bytes(uninstall.OWNER)
        (owned / "user-notes").write_text("keep")
        with self.assertRaises(uninstall.UninstallError):
            self.run_uninstall()
        self.assertTrue(self.manifest.exists())
        self.assertEqual(self.calls, [])

    def test_symlink_candidate_does_not_follow_or_delete_target(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.compat / "tmux-status-mouse.ABC12345").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(uninstall.UninstallError):
            self.run_uninstall()
        self.assertTrue(outside.exists())
        self.assertTrue(self.manifest.exists())

    def test_legacy_build_requires_verified_archive_and_exact_layout(self):
        owned = self.build(legacy=True)
        with self.assertRaisesRegex(uninstall.UninstallError, "archive differs"):
            self.run_uninstall()
        with mock.patch.object(uninstall, "TMUX_SHA256", hashlib.sha256(b"fixture archive").hexdigest()):
            self.run_uninstall()
        self.assertFalse(owned.exists())

    def test_marked_failed_build_can_be_cleaned(self):
        owned = self.build()
        (owned / "build").rmdir()
        self.run_uninstall()
        self.assertFalse(owned.exists())

    def test_external_registration_is_not_uninstalled(self):
        self.build()
        self.manifest.write_text(json.dumps({"format": 3, "external": {"active": True}}))
        with self.assertRaisesRegex(uninstall.UninstallError, "not Homebrew/Cargo"):
            self.run_uninstall()
        self.assertEqual(self.calls, [])

    def test_missing_manifest_never_executes_unowned_app(self):
        self.manifest.unlink()
        owned = self.build()
        self.run_uninstall()
        self.assertFalse(owned.exists())
        self.assertEqual(self.calls, [])

    def test_installed_uninstaller_fallback_requires_recorded_checksum(self):
        self.app.unlink()
        payload = b"owned fixture executable"
        digest = hashlib.sha256(payload).hexdigest()
        expected = self.data / "releases" / digest / "agent-float-term"
        expected.parent.mkdir(parents=True)
        expected.write_bytes(payload)
        expected.chmod(0o700)
        self.app = self.home / ".local/bin/agent-float-term"
        self.app.parent.mkdir(parents=True)
        self.app.symlink_to(expected)
        self.manifest.write_text(json.dumps({"format": 2, "current": digest}))
        expected.write_bytes(b"modified")
        with self.assertRaisesRegex(uninstall.UninstallError, "uninstaller was modified"):
            self.run_uninstall()
        self.assertEqual(self.calls, [])
        expected.write_bytes(payload)
        self.run_uninstall()
        self.assertEqual(self.calls, [["uninstall"], ["uninstall", "--yes"]])

    def test_replaced_build_is_not_deleted(self):
        owned = self.build()
        def replace():
            owned.rename(self.compat / "saved-original")
            self.build()
        self.on_apply = replace
        with self.assertRaisesRegex(uninstall.UninstallError, "directory changed"):
            self.run_uninstall()
        self.assertTrue(owned.exists())
        self.assertTrue((self.compat / "saved-original").exists())

    def test_nested_symlink_is_unlinked_without_removing_external_files(self):
        owned = self.build()
        outside = self.root / "outside"
        outside.mkdir()
        note = outside / "notes"
        note.write_text("keep")
        (owned / "build/link").symlink_to(outside, target_is_directory=True)
        self.run_uninstall()
        self.assertFalse(owned.exists())
        self.assertEqual(note.read_text(), "keep")

    @unittest.skipUnless(shutil.which("lsof"), "real open-file check requires lsof")
    def test_real_process_using_build_is_refused(self):
        owned = self.build()
        with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], cwd=owned,
                              stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL) as child:
            try:
                with self.assertRaisesRegex(uninstall.UninstallError, "in use"):
                    uninstall.idle(owned)
            finally:
                child.terminate()
                child.wait(timeout=5)
        uninstall.idle(owned)

    @unittest.skipUnless(os.environ.get("AFT_UNINSTALL_TEST_BINARY"), "native integration requires an explicit test binary")
    def test_make_target_with_real_managed_installation(self):
        source = Path(os.environ["AFT_UNINSTALL_TEST_BINARY"]).resolve()
        self.manifest.unlink()
        self.manifest.parent.chmod(0o700)
        self.data.chmod(0o700)
        env = {**os.environ, "HOME": str(self.home), "XDG_DATA_HOME": str(self.data.parent),
               "XDG_STATE_HOME": str(self.home / "state"), "XDG_CONFIG_HOME": str(self.home / "config"),
               "XDG_CACHE_HOME": str(self.home / "cache"), "XDG_RUNTIME_DIR": str(self.home / "runtime")}
        for name in ("TMUX", "TMUX_PANE", "AFT_TMUX_BINARY"):
            env.pop(name, None)
        result = subprocess.run([str(source), "install", "--yes"], env=env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        owned = self.build()
        other = self.compat / "tmux-3.5a"
        other.mkdir()
        installed = self.home / ".local/bin/agent-float-term"
        for dry in ("1", "0"):
            result = subprocess.run(["make", "-C", str(uninstall.ROOT), "uninstall", f"DRY_RUN={dry}"],
                                    env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(installed.exists(), dry == "1")
            self.assertEqual(self.manifest.exists(), dry == "1")
            self.assertEqual(owned.exists(), dry == "1")
            self.assertTrue(other.exists())


if __name__ == "__main__":
    unittest.main()
