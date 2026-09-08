"""Explicitly approved stable publication. No tag creation, asset replacement, or OIDC."""

import base64
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid
import zipfile

import release_source

ROOT = Path(__file__).resolve().parent.parent
REPO = "vuongvm812/agent-float-term"
TAP = "vuongvm812/homebrew-tap"
FORMULA = "Formula/agent-float-term.rb"
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu")
TIMEOUT = 3600


class ReleaseError(Exception):
    """A safe, authored diagnostic, never raw subprocess/network output."""


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def package_version(text, lock=False):
    # Match the same explicit metadata style as release_source.resolve; reject inheritance.
    header = r"\[\[package\]\]" if lock else r"\[package\]"
    packages = re.findall(rf"(?ms)^{header}\s*\n(.*?)(?=^\[|\Z)", text)
    versions = [re.findall(r'^version\s*=\s*"([^"\n]+)"\s*$', p, re.M)
                for p in packages if re.findall(r'^name\s*=\s*"([^"\n]+)"\s*$', p, re.M) == ["agent-float-term"]]
    require(len(versions) == 1 and len(versions[0]) == 1, "expected explicit agent-float-term package version")
    return versions[0][0]


def run(*args, cwd=ROOT, binary=False):
    # Never echo command output on failure: Cargo credentials and remote URLs may be secret.
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=not binary, timeout=TIMEOUT,
                            env={**os.environ, "GIT_OPTIONAL_LOCKS": "0", "GH_HOST": "github.com"})
    require(result.returncode == 0, f"{args[0]} {args[1]} failed (output withheld); inspect locally")
    return result.stdout if binary else result.stdout.strip()


def api(path):
    return json.loads(run("gh", "api", f"repos/{REPO}/{path}".rstrip("/")))


def get_release(tag):
    # The single-tag REST endpoint can return 404 for drafts. List with authentication.
    pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{REPO}/releases?per_page=100"))
    matches = [r for page in pages for r in page if r["tag_name"] == tag]
    require(len(matches) <= 1, "ambiguous releases for tag")
    return matches[0] if matches else None


def release_identity(release):
    if release is None:
        return None
    fields = ("id", "tag_name", "target_commitish", "draft", "prerelease", "body", "name")
    asset_fields = ("id", "name", "size", "digest", "updated_at")
    return ([release.get(k) for k in fields],
            sorted(tuple(a.get(k) for k in asset_fields) for a in release["assets"]))


def checkout_state(path, repository, default=False):
    require(Path(run("git", "rev-parse", "--show-toplevel", cwd=path)).resolve() == path.resolve(),
            "checkout path must be the repository root")
    require(not run("git", "status", "--porcelain", "--untracked-files=all", cwd=path),
            "checkout must be completely clean (including staged and untracked files)")
    branch = run("git", "symbolic-ref", "--short", "HEAD", cwd=path)
    allowed = {f"https://github.com/{repository}{suffix}" for suffix in ("", ".git")}
    allowed |= {f"git@github.com:{repository}.git", f"ssh://git@github.com/{repository}.git"}
    for option in ((), ("--push",)):
        urls = run("git", "remote", "get-url", *option, "--all", "origin", cwd=path).splitlines()
        require(len(urls) == 1 and urls[0] in allowed, "origin must be the intended GitHub repository")
    url = run("git", "remote", "get-url", "--push", "origin", cwd=path)
    if default:
        advertised = run("git", "ls-remote", "--symref", url, "HEAD", cwd=path)
        require(f"ref: refs/heads/{branch}\tHEAD" in advertised.splitlines(),
                "tap must be on its remote default branch")
    refs = run("git", "ls-remote", url, f"refs/heads/{branch}", cwd=path).splitlines()
    require(len(refs) == 1, "remote branch missing or ambiguous")
    return branch, url, refs[0].split()[0], run("git", "rev-parse", "HEAD", cwd=path)


def tap_state(path, tag, source, formula=None):
    destination = path / FORMULA
    require(not destination.is_symlink() and not destination.parent.is_symlink()
            and (not destination.exists() or destination.is_file()), "unsafe tap formula path")
    branch, url, remote, head = checkout_state(path, TAP, default=True)
    if head != remote:
        # Only a single, precisely owned commit is resumable. Never push unrelated history.
        require(run("git", "rev-list", "--parents", "-n", "1", head, cwd=path).split() == [head, remote],
                "tap is behind/diverged or has unrelated unpushed commits; reconcile manually")
        message = run("git", "show", "-s", "--format=%B", head, cwd=path)
        content = run("git", "show", f"{head}:{FORMULA}", cwd=path, binary=True)
        require(message == tap_message(tag, source, content), "unowned tap commit; reconcile manually")
        require(run("git", "diff-tree", "--no-commit-id", "--name-only", "-r", head, cwd=path) == FORMULA,
                "tap commit changes more than the release formula")
        require(run("git", "ls-tree", head, "--", FORMULA, cwd=path).startswith("100644 blob "),
                "tap formula must be a regular nonexecutable file")
        if formula is not None:
            require(content == formula, "unpushed tap formula differs from verified release")
    published = destination.read_bytes() if head == remote and destination.exists() else None
    if head != remote and run("git", "ls-tree", remote, "--", FORMULA, cwd=path):
        published = run("git", "show", f"{remote}:{FORMULA}", cwd=path, binary=True)
    if published is not None:
        number = rb"(?:0|[1-9][0-9]*)"
        versions = re.findall(rb"(?m)^[ \t]*version[ \t]+[\"'](" + number + rb"\." + number + rb"\." + number + rb")[\"'][ \t]*$", published)
        require(len(versions) == 1, "tap formula needs one explicit stable version; review manually")
        old, new = tuple(map(int, versions[0].split(b"."))), tuple(map(int, tag[1:].split(".")))
        require(old <= new, "tap already publishes a newer version; refusing downgrade, review manually")
        require(formula is None or old < new or published == formula,
                "published tap formula has the same version but different bytes; review manually")
    return branch, url, remote, head


def tap_message(tag, source, formula):
    return f"release: agent-float-term {tag}\n\nSource: {source['commit']}\nFormula-SHA256: {hashlib.sha256(formula).hexdigest()}"


def fetch(url):
    with urlopen(Request(url, headers={"User-Agent": "agent-float-term-release/1"}), timeout=60) as response:
        return response.read()


def crate_exists(version, commit):
    try:
        record = json.loads(fetch(f"https://crates.io/api/v1/crates/agent-float-term/{version}"))["version"]
    except HTTPError as error:
        if error.code == 404:
            return False
        raise ReleaseError("crates.io version lookup failed") from None
    require(record["num"] == version and not record["yanked"], "crate version mismatch or yanked")
    data = fetch(f"https://static.crates.io/crates/agent-float-term/agent-float-term-{version}.crate")
    require(hashlib.sha256(data).hexdigest() == record["checksum"], "published crate checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        members = [m for m in archive.getmembers() if m.name == f"agent-float-term-{version}/.cargo_vcs_info.json"]
        require(len(members) == 1 and members[0].isfile(), "crate lacks unique source provenance")
        vcs = json.load(archive.extractfile(members[0]))
    require(vcs["git"]["sha1"] == commit and vcs["git"].get("dirty", False) is False
            and vcs.get("path_in_vcs") == "", "published crate does not match clean tagged source")
    return True


def confirm(tag):
    print("Approval accepts the release checklist: reviewed tagged source, changelog, native CI/tests, "
          "and stable release readiness. Authorizes draft CI dispatch if needed, verified GitHub "
          "publication, formula-only tap commit/push, and local Cargo publication using existing credentials.")
    expected = f"publish {tag}"
    answer = os.environ.get("CONFIRM", "")
    if not answer and sys.stdin.isatty():
        answer = input(f"Type exactly '{expected}': ")
    require(answer == expected, "publication not confirmed; no changes made")


def release_runs():
    pages = json.loads(run("gh", "api", "--paginate", "--slurp",
                           f"repos/{REPO}/actions/workflows/release.yml/runs?event=workflow_dispatch&per_page=100"))
    return [item for page in pages for item in page["workflow_runs"]]


def wait_run(tag, request, branch, run_id=None):
    title = f"Release {tag} {request}"
    deadline = time.monotonic() + TIMEOUT
    if run_id is not None:
        print(f"Reusing CI: https://github.com/{REPO}/actions/runs/{run_id}")
    while time.monotonic() < deadline:
        if run_id is None:
            runs = release_runs()
            matches = [r for r in runs if r["display_title"] == title and r["head_branch"] == branch]
            require(len(matches) <= 1, "ambiguous workflow correlation; inspect Actions manually")
            if matches:
                run_id = matches[0]["id"]
                print(f"CI: https://github.com/{REPO}/actions/runs/{run_id}")
        if run_id is not None:
            result = api(f"actions/runs/{run_id}")
            if result["status"] == "completed":
                require(result["conclusion"] == "success", "Release CI failed; inspect the run before resuming")
                return run_id
        time.sleep(10)
    raise ReleaseError("timed out waiting for correlated Release CI; inspect Actions before resuming")


def verify_release(release, tag, source, snapshot, assets, evidence=None):
    require(release and release["tag_name"] == tag and not release["prerelease"]
            and release["target_commitish"] == source["commit"], "release must target exact stable source commit")
    names = [f"agent-float-term-{tag}-{target}.tar.gz" for target in TARGETS] + ["SHA256SUMS", "agent-float-term.rb"]
    require(sorted(a["name"] for a in release["assets"]) == sorted(names), "release asset set is incomplete or unexpected")
    proof_error = "immutable Release proof missing, expired, or invalid; manual review/new prepared release required"
    if evidence is None:
        # Editable notes locate a run; only its immutable artifact can authorize asset bytes.
        marker = (f"Release source: {source['commit']}\nRelease tag object: {source['tag_object']}\n"
                  f"Release verification: https://github.com/{REPO}/actions/runs/")
        ids = re.findall(re.escape(marker) + r"([0-9]+)(?:\n|$)", release.get("body") or "")
        require(len(ids) == 1, proof_error)
        evidence = ids[0]
    require(re.fullmatch(r"[1-9][0-9]*", str(evidence)), proof_error)
    verification = api(f"actions/runs/{evidence}")
    require(verification["id"] == int(evidence) and verification["repository"]["full_name"] == REPO
            and verification["conclusion"] == "success" and verification["status"] == "completed"
            and verification["event"] == "workflow_dispatch" and verification["path"] == ".github/workflows/release.yml"
            and verification["display_title"].split()[:2] == ["Release", tag], proof_error)
    listing = api(f"actions/runs/{evidence}/artifacts?per_page=100")
    proofs = [a for a in listing["artifacts"] if a["name"] == "release-verification"]
    require(listing["total_count"] == len(listing["artifacts"]) and len(proofs) == 1, proof_error)
    proof = proofs[0]
    require(proof["expired"] is False and 0 < proof["size_in_bytes"] <= 65536
            and proof["workflow_run"]["id"] == int(evidence)
            and proof["workflow_run"]["head_sha"] == verification["head_sha"], proof_error)
    data = run("gh", "api", f"repos/{REPO}/actions/artifacts/{proof['id']}/zip", binary=True)
    require(len(data) <= 65536 and proof["digest"] == "sha256:" + hashlib.sha256(data).hexdigest(), proof_error)
    def unique_object(pairs):
        require(len(dict(pairs)) == len(pairs), proof_error)
        return dict(pairs)
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            members = archive.infolist()
            require(len(members) == 1 and members[0].filename == "release-verification.json"
                    and members[0].file_size <= 16384 and not members[0].flag_bits & 1
                    and stat.S_IFMT(members[0].external_attr >> 16) in (0, stat.S_IFREG), proof_error)
            record = json.loads(archive.read(members[0]).decode("utf-8"), object_pairs_hook=unique_object)
    except (ValueError, zipfile.BadZipFile, RuntimeError):
        raise ReleaseError(proof_error) from None
    hashes = {}
    for asset in release["assets"]:
        data = run("gh", "api", "-H", "Accept: application/octet-stream",
                   f"repos/{REPO}/releases/assets/{asset['id']}", binary=True)
        digest = asset.get("digest")
        require(not digest or digest == "sha256:" + hashlib.sha256(data).hexdigest(), "GitHub asset digest mismatch")
        hashes[asset["name"]] = hashlib.sha256(data).hexdigest()
        (assets / asset["name"]).write_bytes(data)
    require(record == {"tag": tag, "commit": source["commit"], "tag_object": source["tag_object"],
                       "run_id": str(evidence), "run_attempt": str(verification["run_attempt"]), "sha256": hashes},
            "release assets/source differ from immutable Release build proof; manual review required")
    print(f"Verified immutable Release proof: https://github.com/{REPO}/actions/runs/{evidence}")
    generated = run(sys.executable, "-B", str(snapshot / "scripts/generate-homebrew-formula.py"),
                    tag, str(assets / "SHA256SUMS"), cwd=snapshot, binary=True)
    formula = (assets / "agent-float-term.rb").read_bytes()
    require(generated == formula, "uploaded formula differs from tagged generator/template or archives")
    return formula


def publish_tap(path, tag, source, formula):
    branch, url, remote, head = tap_state(path, tag, source, formula)
    destination = path / FORMULA
    require(not destination.is_symlink() and not destination.parent.is_symlink(), "unsafe tap formula path")
    if head == remote and destination.exists() and destination.read_bytes() == formula:
        print("Tap: identical formula already pushed; skipped")
        return
    if head == remote:
        destination.parent.mkdir(exist_ok=True)
        destination.write_bytes(formula)
        destination.chmod(0o644)
        run("git", "add", "--", FORMULA, cwd=path)
        require(run("git", "diff", "--cached", "--name-only", cwd=path) == FORMULA, "unrelated staged tap changes")
        run("git", "commit", "--only", "-m", tap_message(tag, source, formula), "--", FORMULA, cwd=path)
    branch2, url2, remote2, head2 = tap_state(path, tag, source, formula)
    require((branch2, url2, remote2) == (branch, url, remote), "tap remote advanced; reconcile manually")
    # Explicit object/ref and a normal push: remote advancement cannot send unrelated commits.
    run("git", "push", "--no-follow-tags", "--recurse-submodules=no", url, f"{head2}:refs/heads/{branch}", cwd=path)
    print(f"Tap: https://github.com/{TAP}/commit/{head2}")


def main():
    os.chdir(ROOT)
    tag = os.environ.get("TAG") or "v" + package_version((ROOT / "Cargo.toml").read_text())
    version = release_source.validate_tag(tag, stable=True)
    dry = os.environ.get("DRY_RUN", "")
    require(dry in ("", "0", "1"), "DRY_RUN must be 0 or 1")
    tap = Path(os.environ.get("TAP_DIR") or ROOT.parent / "homebrew-tap").expanduser().resolve()
    # Capture resolver output too, including subprocess stderr, using the existing CLI.
    try:
        resolved = run(sys.executable, "-B", str(ROOT / "scripts/release_source.py"), "resolve", tag)
    except (ReleaseError, OSError, subprocess.SubprocessError):
        raise ReleaseError(f"cannot resolve {tag}: the local tag must exist and its tagged Cargo.toml "
                           f"must name agent-float-term version {version}. Check TAG/package version; "
                           f"do not move an existing release tag. Read-only diagnosis: "
                           f"python3 -B scripts/release_source.py resolve {tag}") from None
    source = dict(line.split("=", 1) for line in resolved.splitlines())
    print(f"Source: {tag} ({source['commit']}); tap: {tap}")
    print(f"GitHub: https://github.com/{REPO}/releases/tag/{tag}")
    print(f"Crate: https://crates.io/crates/agent-float-term/{version}")
    print("Plan: clean-checkout/remote checks; approve checklist; isolated tagged Cargo metadata; "
          "verify existing crate provenance or tagged package check + cargo +1.84.1 publish --locked --dry-run; "
          "reuse stable draft/active Release run or dispatch (prerelease=false); verify CI/assets/tagged formula; "
          "publish draft; commit/push only tap formula; publish missing crate; verify provenance.")
    if dry == "1":
        print("DRY_RUN: plan only. Dirty tooling allowed; no temporary files, builds, dispatch, or publication. "
              "Remote, Cargo, assets, and tap checks deferred to the confirmed invocation.")
        return
    local = checkout_state(ROOT, REPO)
    require(local[2] == local[3], "source branch has unpushed commits or is behind remote")
    tap_state(tap, tag, source)
    # Wrapper keeps subprocess stderr private, unlike calling check_remote directly.
    def recheck():
        run(sys.executable, "-B", str(ROOT / "scripts/release_source.py"), "check",
            tag, source["commit"], source["tag_object"], REPO)
        require(checkout_state(ROOT, REPO) == local, "source checkout/remote changed during release")
        tap_state(tap, tag, source)
    recheck()
    require(api("").get("permissions", {}).get("push") is True,
            "GitHub write access is required to see drafts and publish; check gh authentication")
    confirm(tag)
    with tempfile.TemporaryDirectory(prefix="agent-float-term-release-") as directory:
        snapshot, assets = Path(directory) / "source", Path(directory) / "assets"
        assets.mkdir()
        run("git", "clone", "--quiet", "--no-hardlinks", "--no-checkout", str(ROOT), str(snapshot))
        run("git", "checkout", "--quiet", "--detach", source["commit"], cwd=snapshot)
        metadata = json.loads(run("cargo", "+1.84.1", "metadata", "--locked", "--no-deps", "--format-version", "1", cwd=snapshot))
        require([p["version"] for p in metadata["packages"] if p["name"] == "agent-float-term"] == [version],
                "locked Cargo metadata version mismatch")
        require(package_version((snapshot / "Cargo.lock").read_text(), lock=True) == version,
                "Cargo.lock package version mismatch")
        exists = crate_exists(version, source["commit"])
        if not exists:
            if (snapshot / "scripts/verify-cargo-package.sh").is_file():
                run("bash", "scripts/verify-cargo-package.sh", cwd=snapshot)
            run("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io", "--dry-run", cwd=snapshot)
        print("Crate preflight: " + ("existing checksum/provenance verified" if exists else "locked publish dry-run passed"))
        require(not run("git", "status", "--porcelain", "--untracked-files=all", cwd=snapshot), "tagged source became dirty")
        release, evidence = get_release(tag), None
        if release is None:
            active = [item for item in release_runs() if item["status"] != "completed"
                      and item["display_title"].split()[:2] == ["Release", tag]]
            require(len(active) <= 1, "multiple active Release runs for this tag; inspect Actions manually")
            if active:
                evidence = wait_run(tag, "", "", run_id=active[0]["id"])
            else:
                require(get_release(tag) is None, "release appeared during preflight; rerun to reuse it")
                branch = api("")["default_branch"]
                workflow = json.loads(run("gh", "api", f"repos/{REPO}/contents/.github/workflows/release.yml?ref={branch}"))
                require("inputs.request_id" in base64.b64decode(workflow["content"]).decode(),
                        "push orchestration workflow support before dispatching")
                recheck()
                request = uuid.uuid4().hex
                print(f"Dispatch correlation: {request}")
                # Not a distributed transaction: simultaneous helpers still rely on server
                # tag concurrency and the workflow's refusal to overwrite an existing release.
                run("gh", "workflow", "run", "release.yml", "--repo", REPO, "--ref", branch,
                    "-f", f"tag={tag}", "-f", "prerelease=false", "-f", f"request_id={request}")
                evidence = wait_run(tag, request, branch)
            release = get_release(tag)
        formula = verify_release(release, tag, source, snapshot, assets, evidence)
        print("GitHub preflight: exact-source CI, archive bytes/checksums, and tagged formula verified")
        tap_state(tap, tag, source, formula)
        recheck()
        require(release_identity(get_release(tag)) == release_identity(release),
                "release changed during verification; rerun without overwriting")
        if release["draft"]:
            run("gh", "release", "edit", tag, "--repo", REPO, "--draft=false")
            print("GitHub: draft published")
        else:
            print("GitHub: already public; verified, skipped publication")
        recheck()
        publish_tap(tap, tag, source, formula)
        recheck()
        if not crate_exists(version, source["commit"]):
            require(not exists, "previously verified crate disappeared; rerun preflight")
            require(not run("git", "status", "--porcelain", "--untracked-files=all", cwd=snapshot), "tagged source became dirty")
            run("cargo", "+1.84.1", "publish", "--locked", "--registry", "crates-io", cwd=snapshot)
            deadline = time.monotonic() + 120
            while not crate_exists(version, source["commit"]):
                require(time.monotonic() < deadline, "crate indexing timed out; resume to verify, do not blindly republish")
                time.sleep(5)
            print("Crate: published and provenance verified")
        else:
            print("Crate: already published; checksum/provenance verified, skipped")
    print("Release complete.")


if __name__ == "__main__":
    try:
        main()
    except ReleaseError as error:
        print(f"Release stopped: {error}. Earlier reported stages may be public; rerun the same TAG "
              "to resume after resolving the blocker. No tags/assets are overwritten.", file=sys.stderr)
        sys.exit(1)
    except (ValueError, OSError, KeyError, EOFError, KeyboardInterrupt, subprocess.SubprocessError, tarfile.TarError):
        # Do not expose exception strings containing credentials or subprocess output.
        print("Release stopped. Earlier reported stages may be public; rerun the same TAG to resume. "
              "No tags/assets are overwritten. Inspect local preflight/CI state if blocked.", file=sys.stderr)
        sys.exit(1)
