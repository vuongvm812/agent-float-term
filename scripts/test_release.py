"""Offline orchestration fixtures. Git/Cargo/GitHub mutations are always mocked."""

import base64
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from unittest.mock import patch
from urllib.error import HTTPError

import release as r

TAG = "v0.2.2"
SHA = "a" * 40
SOURCE = {"commit": SHA, "tag_object": "b" * 40, "version": "0.2.2"}
CONTENT = b'  version "0.2.2"\n# verified formula\n'
STATE = ("main", f"git@github.com:{r.TAP}.git", "c" * 40, "c" * 40)
MANIFEST = '[package]\nname = "agent-float-term"\nversion = "0.2.2"\n'


def draft():
    names = [f"agent-float-term-{TAG}-{t}.tar.gz" for t in r.TARGETS] + ["SHA256SUMS", "agent-float-term.rb"]
    return {"id": 1, "tag_name": TAG, "target_commitish": SHA, "prerelease": False,
            "draft": True, "assets": [{"id": i, "name": name} for i, name in enumerate(names)]}


def verification_fixture(record_change=None, zip_bytes=None):
    release = draft()
    release["body"] = (f"Release source: {SHA}\nRelease tag object: {SOURCE['tag_object']}\n"
                       f"Release verification: https://github.com/{r.REPO}/actions/runs/123\n\nNotes")
    bodies = {asset["id"]: CONTENT for asset in release["assets"]}
    record = {"tag": TAG, "commit": SHA, "tag_object": SOURCE["tag_object"],
              "run_id": "123", "run_attempt": "1", "sha256": {
                  asset["name"]: hashlib.sha256(CONTENT).hexdigest() for asset in release["assets"]}}
    if record_change:
        record_change(record)
    if zip_bytes is None:
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("release-verification.json", json.dumps(record))
        zip_bytes = buffer.getvalue()
    verification = {"id": 123, "repository": {"full_name": r.REPO}, "conclusion": "success",
                    "status": "completed", "event": "workflow_dispatch", "run_attempt": 1,
                    "path": ".github/workflows/release.yml", "display_title": f"Release {TAG} unique",
                    "head_sha": "c" * 40}
    listing = {"total_count": 1, "artifacts": [{"id": 456, "name": "release-verification",
                "expired": False, "size_in_bytes": len(zip_bytes),
                "digest": "sha256:" + hashlib.sha256(zip_bytes).hexdigest(),
                "workflow_run": {"id": 123, "head_sha": verification["head_sha"]}}]}
    replies = {"actions/runs/123": verification, "actions/runs/123/artifacts?per_page=100": listing}

    def command(*args, **kwargs):
        if args[0] == r.sys.executable:
            return CONTENT
        if args[-1].endswith("/zip"):
            return zip_bytes
        return bodies[int(args[-1].rsplit("/", 1)[-1])]

    return release, replies, command, bodies


class MainTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "Cargo.toml").write_text(MANIFEST)
        self.commands = []
        self.env = self.start(patch.dict(os.environ, {"TAG": "", "CONFIRM": f"publish {TAG}", "DRY_RUN": "0", "TAP_DIR": str(self.root / "tap")}))
        self.start(patch.object(r, "ROOT", self.root))
        self.start(patch.object(r.os, "chdir"))
        self.start(patch("builtins.print"))
        self.run = self.start(patch.object(r, "run", side_effect=self.command))
        self.api = self.start(patch.object(r, "api", return_value={"permissions": {"push": True}, "default_branch": "main"}))
        self.checkout = self.start(patch.object(r, "checkout_state", return_value=STATE))
        self.tapstate = self.start(patch.object(r, "tap_state", return_value=STATE))
        self.crate = self.start(patch.object(r, "crate_exists", return_value=True))
        self.release = self.start(patch.object(r, "get_release", return_value=draft()))
        self.runs = self.start(patch.object(r, "release_runs", return_value=[]))
        self.verify = self.start(patch.object(r, "verify_release", return_value=CONTENT))
        self.tap = self.start(patch.object(r, "publish_tap"))

    def start(self, patcher):
        result = patcher.start()
        self.addCleanup(patcher.stop)
        return result

    def command(self, *args, **kwargs):
        self.commands.append(args)
        if "resolve" in args:
            return "\n".join(f"{k}={v}" for k, v in SOURCE.items())
        if args[:2] == ("git", "clone"):
            snapshot = Path(args[-1])
            snapshot.mkdir()
            (snapshot / "Cargo.lock").write_text(MANIFEST.replace("[package]", "[[package]]"))
            (snapshot / "scripts").mkdir()
            (snapshot / "scripts/verify-cargo-package.sh").write_text("# fixture: never executed\n")
        if "metadata" in args:
            return json.dumps({"packages": [{"name": "agent-float-term", "version": "0.2.2"}]})
        if "contents/.github/workflows/release.yml?ref=main" in args[-1]:
            return json.dumps({"content": base64.b64encode(b"inputs.request_id").decode()})
        return ""

    def test_existing_draft_and_crate_publish_only_github_and_tap(self):
        r.main()
        self.assertIn(("gh", "release", "edit", TAG, "--repo", r.REPO, "--draft=false"), self.commands)
        self.tap.assert_called_once()
        self.assertFalse(any("publish" in c for c in self.commands))
        self.assertFalse(any(c[:3] == ("gh", "workflow", "run") for c in self.commands))
        self.assertTrue(any(c[:3] == ("git", "checkout", "--quiet") and SHA in c for c in self.commands))

    def test_already_public_is_verified_not_edited(self):
        release = draft()
        release["draft"] = False
        self.release.return_value = release
        r.main()
        self.verify.assert_called_once()
        self.assertFalse(any("edit" in c for c in self.commands))

    def test_missing_crate_dry_run_precedes_public_effects(self):
        self.crate.side_effect = [False, False, True]
        r.main()
        dry = self.commands.index(("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io", "--dry-run"))
        public = next(i for i, c in enumerate(self.commands) if "edit" in c)
        publish = self.commands.index(("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io"))
        self.assertLess(self.commands.index(("bash", "scripts/verify-cargo-package.sh")), dry)
        self.assertLess(dry, public)
        self.assertLess(public, publish)

    def test_crate_race_checks_provenance_and_skips_publish(self):
        self.crate.side_effect = [False, True]
        r.main()
        self.assertNotIn(("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io"), self.commands)

    def test_missing_release_correlates_dispatch(self):
        self.release.side_effect = [None, None, draft(), draft()]
        self.runs.return_value = [{"display_title": "Release v0.2.20 other", "status": "in_progress"},
                                  {"display_title": f"Release {TAG} finished", "status": "completed"}]
        with patch.object(r, "wait_run", return_value=123) as wait:
            r.main()
        dispatch = next(c for c in self.commands if c[:3] == ("gh", "workflow", "run"))
        self.assertIn("prerelease=false", dispatch)
        self.assertIn("request_id=" + wait.call_args.args[1], dispatch)
        self.assertEqual(self.verify.call_args.args[-1], 123)

    def test_active_exact_tag_run_is_reused_without_dispatch(self):
        matching = {"id": 123, "display_title": f"Release {TAG} earlier", "status": "in_progress"}
        for status in ("queued", "in_progress", "waiting", "pending", "requested"):
            self.commands.clear()
            self.release.side_effect = [None, draft(), draft()]
            self.runs.return_value = [{**matching, "status": status},
                                      {**matching, "id": 456, "display_title": "Release v0.2.20 other"},
                                      {**matching, "id": 789, "status": "completed"}]
            with patch.object(r, "wait_run", return_value=123) as wait:
                r.main()
            wait.assert_called_once_with(TAG, "", "", run_id=123)
            self.assertFalse(any(c[:3] == ("gh", "workflow", "run") for c in self.commands))

    def test_ambiguous_active_runs_or_appearing_draft_never_dispatch(self):
        matching = {"id": 123, "display_title": f"Release {TAG} earlier", "status": "queued"}
        for ambiguous in (True, False):
            self.commands.clear()
            self.release.side_effect = [None, draft()]
            self.runs.return_value = [matching, {**matching, "id": 456}] if ambiguous else []
            with patch.object(r, "wait_run") as wait, self.assertRaises(r.ReleaseError):
                r.main()
            wait.assert_not_called()
            self.assertFalse(any(c[:3] == ("gh", "workflow", "run") for c in self.commands))

    def test_resolver_failure_explains_tag_version_and_read_only_diagnosis(self):
        self.run.side_effect = r.ReleaseError("private token https://secret@github.com/repo")
        with self.assertRaises(r.ReleaseError) as error:
            r.main()
        message = str(error.exception)
        self.assertIn("local tag must exist", message)
        self.assertIn("agent-float-term version 0.2.2", message)
        self.assertIn(f"python3 -B scripts/release_source.py resolve {TAG}", message)
        self.assertNotIn("secret", message)
        self.assertNotIn("token", message)
        self.checkout.assert_not_called()

    def test_dirty_dry_run_only_resolves_local_tag(self):
        os.environ["DRY_RUN"] = "1"
        self.checkout.side_effect = AssertionError("must not require clean tooling")
        with patch.object(r.tempfile, "TemporaryDirectory", side_effect=AssertionError("no writes")):
            r.main()
        self.assertEqual(len(self.commands), 1)
        self.assertIn("resolve", self.commands[0])
        self.crate.assert_not_called()
        self.release.assert_not_called()

    def test_tag_override_does_not_use_head_version(self):
        (self.root / "Cargo.toml").write_text(MANIFEST.replace("0.2.2", "0.3.0"))
        os.environ.update(TAG=TAG, DRY_RUN="1")
        r.main()
        self.assertEqual(self.commands[0][-1], TAG)

    def test_decline_non_tty_and_wrong_confirm_have_no_mutations(self):
        for answer in ("", "yes", "publish v0.2.1", f"publish {TAG} "):
            self.commands.clear()
            os.environ["CONFIRM"] = answer
            with patch.object(r.sys.stdin, "isatty", return_value=False), self.assertRaises(r.ReleaseError):
                r.main()
            self.assertFalse(any(c[0] in ("git", "cargo", "gh") for c in self.commands))
            self.release.assert_not_called()

    def test_failed_preflight_has_no_mutations(self):
        self.checkout.side_effect = r.ReleaseError("dirty")
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.assertEqual(len(self.commands), 1)

    def test_read_only_github_access_never_dispatches_hidden_draft(self):
        self.api.return_value = {"permissions": {"push": False}}
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.release.assert_not_called()
        self.assertFalse(any(c[0] in ("git", "cargo", "gh") for c in self.commands))

    def test_source_ahead_or_tag_moved_refuses_before_approval(self):
        self.checkout.return_value = (*STATE[:3], "d" * 40)
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.checkout.return_value = STATE
        original = self.command
        def command(*args, **kwargs):
            if "check" in args:
                raise r.ReleaseError("remote tag moved")
            return original(*args, **kwargs)
        self.run.side_effect = command
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.release.assert_not_called()

    def test_invalid_tag_and_dry_run_values_never_reach_commands(self):
        for tag in ("v0.2.2;false", "v0.2.2-rc.1", "v00.2.2", "$(touch /never)"):
            os.environ["TAG"] = tag
            with self.assertRaises(ValueError):
                r.main()
        os.environ.update(TAG=TAG, DRY_RUN="true")
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.assertEqual(self.commands, [])

    def test_failed_cargo_dry_run_has_no_public_effects(self):
        self.crate.return_value = False
        original = self.command
        def command(*args, **kwargs):
            if "--dry-run" in args:
                raise r.ReleaseError("Cargo verification failed")
            return original(*args, **kwargs)
        self.run.side_effect = command
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.release.assert_not_called()
        self.tap.assert_not_called()

    def test_failed_tagged_package_inspection_has_no_public_effects(self):
        self.crate.return_value = False
        original = self.command
        def command(*args, **kwargs):
            if args[0] == "bash":
                raise r.ReleaseError("package contents rejected")
            return original(*args, **kwargs)
        self.run.side_effect = command
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.release.assert_not_called()
        self.assertFalse(any("publish" in c for c in self.commands))

    def test_missing_optional_tagged_package_inspector_retains_cargo_dry_run(self):
        self.crate.side_effect = [False, True]
        with patch.object(Path, "is_file", return_value=False):
            r.main()
        self.assertNotIn(("bash", "scripts/verify-cargo-package.sh"), self.commands)
        self.assertTrue(any("--dry-run" in c for c in self.commands))

    def test_same_version_tap_collision_stops_before_github_publication(self):
        def state(*args):
            if len(args) == 4:
                raise r.ReleaseError("same version but different bytes")
            return STATE
        self.tapstate.side_effect = state
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.assertFalse(any("edit" in c for c in self.commands))
        self.tap.assert_not_called()

    def test_failed_new_release_ci_does_not_publish(self):
        self.release.return_value = None
        with patch.object(r, "wait_run", side_effect=r.ReleaseError("CI failed")), self.assertRaises(r.ReleaseError):
            r.main()
        self.verify.assert_not_called()
        self.tap.assert_not_called()

    def test_bad_assets_or_racing_release_prevents_publication(self):
        for bad_assets in (True, False):
            self.commands.clear()
            self.verify.side_effect = r.ReleaseError("bad assets") if bad_assets else None
            changed = draft()
            changed["assets"][0]["id"] = 100
            self.release.side_effect = [draft(), changed]
            with self.assertRaises(r.ReleaseError):
                r.main()
            self.assertFalse(any("edit" in c for c in self.commands))
            self.tap.assert_not_called()

    def test_tap_failure_after_github_does_not_publish_crate(self):
        self.crate.return_value = False
        self.tap.side_effect = r.ReleaseError("push failed")
        with self.assertRaises(r.ReleaseError):
            r.main()
        self.assertTrue(any("edit" in c for c in self.commands))
        self.assertNotIn(("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io"), self.commands)

    def test_bad_lock_or_metadata_prevents_dispatch_and_publication(self):
        original = self.command
        for bad in ("lock", "metadata"):
            self.commands.clear()
            def command(*args, **kwargs):
                result = original(*args, **kwargs)
                if bad == "lock" and args[:2] == ("git", "clone"):
                    (Path(args[-1]) / "Cargo.lock").write_text(MANIFEST.replace("[package]", "[[package]]").replace("0.2.2", "0.2.1"))
                return result.replace("0.2.2", "0.2.1") if bad == "metadata" and "metadata" in args else result
            self.run.side_effect = command
            with self.assertRaises(r.ReleaseError):
                r.main()
            self.release.assert_not_called()


class BoundaryTests(unittest.TestCase):
    def setUp(self):
        printer = patch("builtins.print")
        printer.start()
        self.addCleanup(printer.stop)

    def test_bare_make_with_confirmation_only_shows_help(self):
        result = subprocess.run(["make"], cwd=r.ROOT, capture_output=True, text=True,
                                env={**os.environ, "CONFIRM": f"publish {TAG}", "TAG": TAG,
                                     "MAKEFLAGS": "", "MFLAGS": "", "MAKEFILES": ""}, check=True)
        self.assertIn("read-only release plan", result.stdout)
        self.assertNotIn("Source:", result.stdout)

    def test_release_run_lookup_paginates_before_matching(self):
        with patch.object(r, "run", return_value=json.dumps([
            {"workflow_runs": [{"id": 1}]}, {"workflow_runs": [{"id": 2}]}
        ])) as run:
            self.assertEqual(r.release_runs(), [{"id": 1}, {"id": 2}])
            self.assertIn("--paginate", run.call_args.args)

    def test_release_identity_ignores_download_counts_but_not_asset_replacement(self):
        before, after = draft(), draft()
        after["assets"][0]["download_count"] = 5
        self.assertEqual(r.release_identity(before), r.release_identity(after))
        after["assets"][0]["id"] = 9
        self.assertNotEqual(r.release_identity(before), r.release_identity(after))

    def test_argv_and_private_errors(self):
        result = subprocess.CompletedProcess([], 1, "secret-token", "https://token@github.com/x")
        with patch.object(r.subprocess, "run", return_value=result) as command, self.assertRaises(r.ReleaseError) as error:
            r.run("git", "push", "$(touch /never)")
        self.assertNotIn("token", str(error.exception))
        self.assertNotIn("shell", command.call_args.kwargs)
        self.assertEqual(command.call_args.args[0][-1], "$(touch /never)")

    def test_crate_download_checksum_and_clean_exact_provenance(self):
        for sha, dirty, checksum, succeeds in ((SHA, False, True, True), (SHA, True, True, False),
                                                ("b" * 40, False, True, False), (SHA, False, False, False)):
            data = io.BytesIO()
            with tarfile.open(fileobj=data, mode="w:gz") as archive:
                vcs = json.dumps({"git": {"sha1": sha, "dirty": dirty}, "path_in_vcs": ""}).encode()
                member = tarfile.TarInfo("agent-float-term-0.2.2/.cargo_vcs_info.json")
                member.size = len(vcs)
                archive.addfile(member, io.BytesIO(vcs))
            body = data.getvalue()
            record = {"version": {"num": "0.2.2", "yanked": False, "checksum": hashlib.sha256(body).hexdigest() if checksum else "bad"}}
            with patch.object(r, "fetch", side_effect=[json.dumps(record), body]):
                if succeeds:
                    self.assertTrue(r.crate_exists("0.2.2", SHA))
                else:
                    with self.assertRaises(r.ReleaseError):
                        r.crate_exists("0.2.2", SHA)

    def test_only_crates_404_means_missing(self):
        for code in (404, 403, 500):
            with patch.object(r, "fetch", side_effect=HTTPError("url", code, "private", {}, None)):
                if code == 404:
                    self.assertFalse(r.crate_exists("0.2.2", SHA))
                else:
                    with self.assertRaises(r.ReleaseError):
                        r.crate_exists("0.2.2", SHA)

    def test_release_lookup_includes_drafts_and_propagates_auth_errors(self):
        with patch.object(r, "run", return_value=json.dumps([[], [draft()]])):
            self.assertEqual(r.get_release(TAG), draft())
        with patch.object(r, "run", side_effect=r.ReleaseError("auth")), self.assertRaises(r.ReleaseError):
            r.get_release(TAG)

    def test_correlated_run_not_latest_and_bounded_failure(self):
        matching = {"id": 123, "display_title": f"Release {TAG} request", "head_branch": "main"}
        other = {**matching, "id": 456, "display_title": f"Release {TAG} unrelated"}
        with patch.object(r, "release_runs", return_value=[other, matching]), patch.object(r, "api", return_value={"status": "completed", "conclusion": "success"}) as api:
            self.assertEqual(r.wait_run(TAG, "request", "main"), 123)
            self.assertEqual(api.call_args.args[0], "actions/runs/123")
        with patch.object(r, "release_runs", return_value=[matching, matching]), self.assertRaises(r.ReleaseError):
            r.wait_run(TAG, "request", "main")
        with patch.object(r.time, "monotonic", side_effect=[0, r.TIMEOUT + 1]), self.assertRaises(r.ReleaseError):
            r.wait_run(TAG, "request", "main")

    def test_reused_run_is_polled_by_id_even_after_it_completes(self):
        for conclusion in ("success", "failure", "cancelled"):
            with patch.object(r, "release_runs") as runs, patch.object(r, "api", return_value={"status": "completed", "conclusion": conclusion}) as api:
                if conclusion == "success":
                    self.assertEqual(r.wait_run(TAG, "", "", run_id=123), 123)
                else:
                    with self.assertRaises(r.ReleaseError):
                        r.wait_run(TAG, "", "", run_id=123)
                runs.assert_not_called()
                api.assert_called_once_with("actions/runs/123")

    def test_existing_assets_require_exact_ci_and_tagged_generator(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release, replies, command, _ = verification_fixture()
            with patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=command) as run:
                self.assertEqual(r.verify_release(release, TAG, SOURCE, root / "tagged", root), CONTENT)
                self.assertEqual(run.call_args.args[2], str(root / "tagged/scripts/generate-homebrew-formula.py"))
            for mutation in ("prerelease", "target", "assets", "digest", "formula", "ci"):
                release, replies, command, _ = verification_fixture()
                if mutation == "prerelease":
                    release["prerelease"] = True
                if mutation == "target":
                    release["target_commitish"] = "main"
                if mutation == "assets":
                    release["assets"].pop()
                if mutation == "digest":
                    release["assets"][0]["digest"] = "sha256:wrong"
                if mutation == "ci":
                    replies["actions/runs/123"]["conclusion"] = "failure"
                def modified(*args, **kwargs):
                    return b"different" if mutation == "formula" and args[0] == r.sys.executable else command(*args, **kwargs)
                with patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=modified), self.assertRaises(r.ReleaseError):
                    r.verify_release(release, TAG, SOURCE, root / "tagged", root)

    def test_persisted_pipeline_evidence_resumes_without_separate_push_ci(self):
        with tempfile.TemporaryDirectory() as temp:
            for key, value in ((None, None), ("conclusion", "failure"), ("path", "another.yml"),
                               ("display_title", "Release v0.2.1 unrelated"), ("id", 124),
                               ("repository", {"full_name": "another/repo"})):
                release, replies, command, _ = verification_fixture()
                if key:
                    replies["actions/runs/123"][key] = value
                with patch.object(r, "api", side_effect=replies.__getitem__) as api, patch.object(r, "run", side_effect=command):
                    if key:
                        with self.assertRaises(r.ReleaseError):
                            r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp))
                        api.assert_called_once_with("actions/runs/123")
                    else:
                        self.assertEqual(r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp)), CONTENT)
                        self.assertEqual(api.call_count, 2)

    def test_consistently_substituted_assets_still_fail_build_proof(self):
        release, replies, command, bodies = verification_fixture()
        # All current asset digests and the mocked regenerated formula agree with
        # the replacement bytes. Only immutable build evidence detects the change.
        for asset in release["assets"]:
            bodies[asset["id"]] = b"substituted archive/checksum/formula"
            asset["digest"] = "sha256:" + hashlib.sha256(bodies[asset["id"]]).hexdigest()
        def modified(*args, **kwargs):
            return bodies[0] if args[0] == r.sys.executable else command(*args, **kwargs)
        with tempfile.TemporaryDirectory() as temp, patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=modified), self.assertRaisesRegex(r.ReleaseError, "immutable Release build proof"):
            r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp))

    def test_missing_expired_or_changed_proof_is_not_authorized_by_notes(self):
        for mutation in ("notes", "missing", "duplicate", "expired", "digest", "run", "head", "size"):
            release, replies, command, _ = verification_fixture()
            listing = replies["actions/runs/123/artifacts?per_page=100"]
            proof = listing["artifacts"][0]
            if mutation == "notes":
                release.pop("body")
            elif mutation == "missing":
                listing.update(total_count=0, artifacts=[])
            elif mutation == "duplicate":
                listing.update(total_count=2, artifacts=[proof, proof])
            elif mutation == "expired":
                proof["expired"] = True
            elif mutation == "digest":
                proof["digest"] = "sha256:wrong"
            elif mutation == "run":
                proof["workflow_run"]["id"] = 124
            elif mutation == "head":
                proof["workflow_run"]["head_sha"] = "d" * 40
            else:
                proof["size_in_bytes"] = 65537
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp, patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=command), self.assertRaises(r.ReleaseError):
                r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp))

    def test_proof_must_record_exact_tag_source_attempt_and_hashes(self):
        for key, value in (("tag", "v0.2.3"), ("commit", "d" * 40), ("tag_object", "e" * 40),
                           ("run_id", "124"), ("run_attempt", "2"), ("sha256", {})):
            release, replies, command, _ = verification_fixture(lambda record: record.update({key: value}))
            with self.subTest(key=key), tempfile.TemporaryDirectory() as temp, patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=command), self.assertRaises(r.ReleaseError):
                r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp), evidence=123)

    def test_proof_zip_and_json_are_validated_without_extraction(self):
        bad_zips = [b"not a ZIP"]
        for name, contents in (("../release-verification.json", "{}"),
                               ("release-verification.json", '{"tag":1,"tag":2}'),
                               ("release-verification.json", "not JSON"),
                               ("release-verification.json", "x" * 16385)):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as archive:
                archive.writestr(name, contents)
            bad_zips.append(buffer.getvalue())
        for data in bad_zips:
            release, replies, command, _ = verification_fixture(zip_bytes=data)
            with tempfile.TemporaryDirectory() as temp, patch.object(r, "api", side_effect=replies.__getitem__), patch.object(r, "run", side_effect=command), self.assertRaises(r.ReleaseError):
                r.verify_release(release, TAG, SOURCE, Path(temp), Path(temp))


class TapTests(unittest.TestCase):
    def setUp(self):
        printer = patch("builtins.print")
        printer.start()
        self.addCleanup(printer.stop)

    def test_synced_tap_rejects_downgrade_same_version_collision_and_unknown_version(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Formula").mkdir()
            destination = root / r.FORMULA
            for current in (b'version "0.3.0"\n', CONTENT + b"# user changed\n",
                            b'version "0.2.2-rc.1"\n', b"unknown formula", CONTENT + CONTENT):
                destination.write_bytes(current)
                with patch.object(r, "checkout_state", return_value=STATE), patch.object(r, "run") as run, self.assertRaises(r.ReleaseError):
                    r.publish_tap(root, TAG, SOURCE, CONTENT)
                run.assert_not_called()
                self.assertEqual(destination.read_bytes(), current)

    def test_tap_version_preflight_allows_numeric_upgrade_and_identical_skip(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Formula").mkdir()
            destination = root / r.FORMULA
            for old, new in (("0.2.1", TAG), ("0.2.9", "v0.2.10"), ("0.9.9", "v0.10.0")):
                destination.write_text(f'  version "{old}"\n')
                with patch.object(r, "checkout_state", return_value=STATE), patch.object(r, "run") as run:
                    self.assertEqual(r.tap_state(root, new, SOURCE, CONTENT), STATE)
                    run.assert_not_called()
            destination.write_bytes(CONTENT)
            with patch.object(r, "checkout_state", return_value=STATE), patch.object(r, "run") as run:
                r.publish_tap(root, TAG, SOURCE, CONTENT)
                run.assert_not_called()

    def test_symlink_tap_formula_refuses_in_preflight(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Formula").symlink_to(root / "elsewhere")
            with patch.object(r, "checkout_state") as state, self.assertRaises(r.ReleaseError):
                r.tap_state(root, TAG, SOURCE)
            state.assert_not_called()

    def test_remote_preflight_rejects_dirty_wrong_remote_multiple_urls_and_nondefault(self):
        good = [str(Path("tap").resolve()), "", "main", STATE[1], STATE[1], STATE[1], "ref: refs/heads/main\tHEAD\nsha\tHEAD", "c" * 40 + "\trefs/heads/main", "c" * 40]
        with patch.object(r, "run", side_effect=good):
            self.assertEqual(r.checkout_state(Path("tap"), r.TAP, True), STATE)
        for index, value in ((0, "/wrong/root"), (1, "?? unrelated"), (3, "https://secret@github.com/wrong/tap"),
                             (4, STATE[1] + "\nother"), (6, "ref: refs/heads/other\tHEAD")):
            replies = good.copy()
            replies[index] = value
            with patch.object(r, "run", side_effect=replies), self.assertRaises(r.ReleaseError):
                r.checkout_state(Path("tap"), r.TAP, True)

    def test_only_exact_owned_single_formula_commit_resumes(self):
        ahead = (*STATE[:3], "d" * 40)
        good = [f"{ahead[3]} {ahead[2]}", r.tap_message(TAG, SOURCE, CONTENT), CONTENT, r.FORMULA, "100644 blob e\t" + r.FORMULA, ""]
        with patch.object(r, "checkout_state", return_value=ahead), patch.object(r, "run", side_effect=good):
            self.assertEqual(r.tap_state(Path("tap"), TAG, SOURCE, CONTENT), ahead)
        for index, value in ((0, "d c unrelated"), (1, "unrelated commit"), (2, b"wrong formula"),
                             (3, r.FORMULA + "\nREADME.md"), (4, "120000 blob symlink")):
            replies = good.copy()
            replies[index] = value
            with patch.object(r, "checkout_state", return_value=ahead), patch.object(r, "run", side_effect=replies), self.assertRaises(r.ReleaseError):
                r.tap_state(Path("tap"), TAG, SOURCE, CONTENT)

    def test_owned_unpushed_commit_cannot_resume_a_published_formula_downgrade(self):
        ahead = (*STATE[:3], "d" * 40)
        for published in (b'version "0.3.0"\n', CONTENT + b"# user changed\n"):
            replies = [f"{ahead[3]} {ahead[2]}", r.tap_message(TAG, SOURCE, CONTENT), CONTENT,
                       r.FORMULA, "100644 blob e\t" + r.FORMULA, "100644 blob f\t" + r.FORMULA, published]
            with patch.object(r, "checkout_state", return_value=ahead), patch.object(r, "run", side_effect=replies), self.assertRaises(r.ReleaseError):
                r.tap_state(Path("tap"), TAG, SOURCE, CONTENT)

    def test_tap_identical_skip_new_commit_and_resume_push_are_narrow(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "Formula").mkdir()
            file = root / r.FORMULA
            file.write_bytes(CONTENT)
            ahead = (*STATE[:3], "d" * 40)
            with patch.object(r, "tap_state", return_value=STATE), patch.object(r, "run") as run:
                r.publish_tap(root, TAG, SOURCE, CONTENT)
                run.assert_not_called()
            file.write_bytes(b"older formula")
            with patch.object(r, "tap_state", side_effect=[STATE, ahead]), patch.object(r, "run", return_value=r.FORMULA) as run:
                r.publish_tap(root, TAG, SOURCE, CONTENT)
                commands = [c.args for c in run.call_args_list]
                self.assertIn(("git", "add", "--", r.FORMULA), commands)
                self.assertIn(("git", "commit", "--only", "-m", r.tap_message(TAG, SOURCE, CONTENT), "--", r.FORMULA), commands)
                self.assertIn(("git", "push", "--no-follow-tags", "--recurse-submodules=no", STATE[1], f"{ahead[3]}:refs/heads/main"), commands)
            with patch.object(r, "tap_state", return_value=ahead), patch.object(r, "run") as run:
                r.publish_tap(root, TAG, SOURCE, CONTENT)
                self.assertEqual(len(run.call_args_list), 1)
                self.assertEqual(run.call_args.args[1], "push")

    def test_tap_remote_advance_or_staging_collision_never_pushes(self):
        with tempfile.TemporaryDirectory() as temp:
            for collision in (True, False):
                root = Path(temp)
                advanced = (*STATE[:2], "e" * 40, "d" * 40)
                with patch.object(r, "tap_state", side_effect=[STATE, advanced]), patch.object(r, "run", return_value="unrelated" if collision else r.FORMULA) as run, self.assertRaises(r.ReleaseError):
                    r.publish_tap(root, TAG, SOURCE, CONTENT + str(collision).encode())
                self.assertFalse(any(c.args[1] == "push" for c in run.call_args_list))


if __name__ == "__main__":
    unittest.main()
