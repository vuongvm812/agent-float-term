# Publishing

Release archives, the Homebrew tap, and crates.io are separate publication steps.
They must identify the same reviewed source version. Workflows do not create source
commits or tags, publish a stable GitHub draft automatically, or push to the tap.
The local `make release` entry point can complete publication after explicit
maintainer confirmation, including a formula-only tap commit/push. It never
creates or moves application tags or commits application source. This guide does
not authorize an automation agent to publish while implementing or testing tooling.

Cargo v0.2.2 is already published. Current source adds coordinated publication and
first-use Homebrew setup for a new release, not a replacement of old artifacts.
See the [release checklist](release-checklist.md) for acceptance gates and
[installation](installation.md) for user-facing commands.

## Make Release

Use this entry point to coordinate all three channels rather than executing the
manual stages below separately:

```sh
# Local plan only, safe while reviewing uncommitted tooling:
make release DRY_RUN=1

# After preparing and pushing the reviewed version/tag:
make release
```

The tag defaults to `v` plus the current `Cargo.toml` version. Override it with
`TAG=vX.Y.Z` to resume an existing stable release; prereleases are not supported
by this combined Homebrew release command. The selected tag must already exist
locally and remotely and match its tagged `Cargo.toml` and `Cargo.lock`. Fix a
version mismatch before tagging: the command never bumps versions or repairs tags.
It publishes from an isolated clean checkout of that exact commit, even when the
working branch contains newer release tooling.

Prerequisites:

- A clean application checkout on a named branch, synchronized with its intended
  GitHub `origin`, including all release tooling pushed to the trusted default branch.
- A clean tap checkout on its remote default branch, synchronized with
  `vuongvm812/homebrew-tap`. The default location is the application checkout's
  sibling `homebrew-tap`; override with `TAP_DIR=/absolute/path/to/homebrew-tap`.
- Git, Make, Python 3.9+, Rust/Cargo 1.84.1, the native build toolchain, and `gh`.
  GitHub CLI authentication must permit draft access, workflow dispatch, and
  release publication and Actions artifact downloads; Git authentication must
  permit a normal push to the tap.
- Local Cargo publishing credentials configured through its credential provider
  (for example `cargo login`). This command uses local Cargo credentials, **not**
  the separate OIDC workflow. Do not start both publishers for the same version.
- Reviewed release notes and completed release-checklist/platform acceptance.
  Confirmation accepts release readiness; automation does not replace that review.

`make release` asks you to type exactly `publish TAG`. Noninteractive use requires
the same explicit confirmation, for example after approving a new `v0.2.3` tag:

```sh
make release TAG=v0.2.3 CONFIRM='publish v0.2.3'
```

Bare `make` displays help. `DRY_RUN=1` validates the local tag and prints the plan
only: no temporary files, builds, remote checks, dispatches, commits, or uploads.
It does not assert that credentials, remote assets, or the tap are ready.

After confirmation, the command:

1. Checks the immutable remote tag and locked tagged package. A missing crate
   receives package-content verification and a Cargo publish dry run. An existing
   crate is downloaded, checksum-verified, and checked against its recorded clean
   Git source commit; a mismatch or yanked version stops the release.
2. Reuses the tag's release, or dispatches the stable Release workflow and waits
   up to one hour. Unique run IDs correlate dispatches. A single matching active
   run is reused on retry; ambiguous runs stop instead of dispatching another.
3. Requires a successful Release run and its immutable `release-verification`
   Actions artifact, which binds the exact tag/source and hashes of all five
   public assets to that build. Downloads the archives, checksums, and formula,
   verifies those recorded hashes and archive layouts, and compares the formula
   with output from the **tagged** generator/template. Then publishes a verified
   draft, or leaves an already-public verified release unchanged.
4. Commits/pushes only `Formula/agent-float-term.rb` to the tap using a normal push.
   Identical published content is skipped. Downgrades, same-version/different
   formulas, unrelated changes, and diverged history require manual review.
5. Publishes a missing crate with `cargo +1.84.1 publish --locked --registry crates-io`,
   then checks indexing and source provenance. An already-published matching
   version is skipped, never uploaded again.

Publication across services is **not atomic**. Run one coordinator per release.
If a later stage fails, earlier stages may already be public; fix the reported
blocker and rerun the same `TAG`. One unpushed, precisely identified formula-only
commit created by this command can be resumed. Unrelated unpushed commits, failed
hooks leaving edits, or remote advancement require manual reconciliation; nothing
is reset or force-pushed. Concurrent coordinators are not a distributed transaction.

Build evidence is retained for 90 days, subject to repository retention policy.
Missing, expired, or inconsistent proof stops automated publication, even when
editable release notes claim a successful run. Old drafts without that proof
require manual review via the procedure below or a newly prepared release; a
generic successful CI run is not proof of the binaries' origin. An interrupted
workflow after proof upload should be inspected before retrying: rerunning jobs
in the same run does not overwrite its immutable evidence artifact.

The current `v0.2.2` tag and the published 0.2.2 crate recorded different source
commits when checked during development. The coordinator intentionally refuses
that combination. Publish these changes under a new matching version/tag rather
than overwriting an existing crate version or moving its tag.

## Prepare The Source

1. Complete the platform/runtime acceptance checklist and review package contents.
   Confirm `Cargo.toml`, `Cargo.lock`, changelog, and installation docs describe the
   version you intend to publish. Do not label unpublished channels as available.
2. The maintainer commits reviewed source and creates/pushes an immutable tag such
   as `v0.1.0`. Its version must match the tagged `Cargo.toml`. Prerelease tags use
   SemVer, for example `v0.1.0-rc.1`. Build-metadata suffixes are not supported by
   these release helpers. Tag pushes alone publish nothing.
3. Use a trusted default-branch workflow containing these release helpers. Protect
   release tags against changes and restrict publishing-environment approvals.
   Tag rechecks narrow races but cannot make remote tag changes and upload atomic.

In a clean checkout of the intended source, these checks do not publish:

```sh
cargo +1.84.1 fmt --all -- --check
cargo +1.84.1 clippy --locked --all-targets -- -D warnings
cargo +1.84.1 test --locked --all-targets
python3 -B scripts/test_release_helpers.py
bash scripts/verify-cargo-package.sh
cargo +1.84.1 publish --locked --dry-run
```

Package verification inspects Cargo's actual file list and builds the unpacked
crate. It requires source, README, LICENSE, and the binary lockfile, and rejects
local build, distribution, graph, workflow, socket, and Python bytecode artifacts.
It does not prove an authenticated registry upload will succeed. Inspect the
result under `target/package/` before first publication. Do not use `--allow-dirty`
or `--no-verify` for a real release.

## GitHub Archives

Manually dispatch **Release** (`.github/workflows/release.yml`) on the trusted
default branch with the existing tag. It resolves and records both the tag object
and peeled source commit, verifies the tagged Cargo version, runs reusable CI,
and builds these native archives with Rust 1.84.1:

| Target | Build Host |
| --- | --- |
| `aarch64-apple-darwin` | macOS 14 |
| `x86_64-apple-darwin` | macOS 15 Intel |
| `x86_64-unknown-linux-gnu` | Ubuntu 22.04 / glibc 2.35 |

Archive names are `agent-float-term-TAG-TARGET.tar.gz`. Each contains one enclosing
directory with only the binary, LICENSE, and README. `SHA256SUMS` hashes the
archives, not the extracted executables. Archives are not signed/notarized.

The workflow creates a **draft**, refuses to overwrite an existing release, and
rechecks the original tag object and commit immediately before creation. Review
assets, checksums, and native installation/PTY acceptance before manually
publishing. Only stable published releases populate `/releases/latest`.

The `prerelease` input defaults to `true`. A prerelease tag cannot request a stable
draft. Stable drafts (`prerelease: false`) also include a generated
`agent-float-term.rb` formula asset. Prerelease drafts do not generate a stable
formula. If you promote a previously prepared draft, generate/review its formula
from the exact release source and archives as described below; do not rerun the
workflow expecting it to replace an existing release.

## Homebrew Tap

The tap is [vuongvm812/homebrew-tap](https://github.com/vuongvm812/homebrew-tap),
locally `~/Developing/homebrew-tap`. The durable formula template and generator
live in the application source so they are reviewed and versioned with releases:

- `scripts/agent-float-term.rb.in`: non-installable template, tmux dependency,
  platform restrictions, stable-path setup caveats, and an isolated integration test.
- `scripts/generate-homebrew-formula.py`: stable-tag and checksum validation,
  archive hash/layout verification, and final Ruby rendering. It never downloads,
  updates the tap, or substitutes invented checksums.
- `scripts/test-homebrew-formula.rb`: executes the formula's actual test using a
  temporary Cellar/opt layout and a small DSL shim. This is not a full Brew audit.

For each stable release:

1. Review the draft's generated `agent-float-term.rb` and verify its three URLs
   and hashes against the accepted release archives. The workflow uses the exact
   resolved source/template, not a potentially newer default-branch template.
2. Publish the stable GitHub release so the formula URLs resolve publicly.
3. Place the reviewed formula asset in the tap's `Formula/agent-float-term.rb`.
   Review the tap diff and commit/publish it as the maintainer, or use the confirmed
   `make release` coordinator. No GitHub workflow writes cross-repository commits
   or uses a tap write token; the local coordinator uses your Git authentication.
4. In disposable environments with the reviewed tap checkout, run actual Homebrew
   checks before advertising availability:

```sh
brew audit --strict --online vuongvm812/tap/agent-float-term
brew install vuongvm812/tap/agent-float-term
brew test vuongvm812/tap/agent-float-term
brew uninstall vuongvm812/tap/agent-float-term
```

For initial publication, validate with a local tap checkout before publishing the
tap commit. Exercise both formula tests and interactive runtime acceptance on
each supported platform. Do not run these install/uninstall checks against a
user's live package or tmux server. This is a custom binary formula, not a
`homebrew/core` submission or a Homebrew bottle-building pipeline.

Local regeneration is available from the exact release source checkout. Put all
three real archives next to their `SHA256SUMS`, then run:

```sh
python3 -B scripts/generate-homebrew-formula.py v0.1.0 /absolute/path/to/SHA256SUMS \
  --output /absolute/path/to/new-agent-float-term.rb
ruby -c /absolute/path/to/new-agent-float-term.rb
```

Use the actual stable tag. Output must be a new file; existing files and symlinks
are refused. For updates, repeat this process with the new tag and archive hashes,
then test `brew upgrade` and old-keg cleanup. Never move an existing release tag or
replace its archives to change the package behind a published version.

## First Crates.io Publish

Crates.io distributes this program as source for
`cargo install agent-float-term --locked`. It does not distribute the GitHub executable archives.
The crate name is allocated on first successful publication; an earlier empty
lookup is not a reservation. Published versions cannot be overwritten.

Trusted Publishing currently requires an already-published crate. The initial
publication of this project is complete; for a new crate, bootstrap with the
following maintainer procedure or the confirmed local coordinator with a token:

1. Sign in to crates.io, verify your email, and confirm the intended crate name.
   Create a short-lived API token authorized for the initial publication. Do not
   paste credentials into chat, source files, or shell command arguments.
2. Authenticate through Cargo's credential provider (for example, interactive
   `cargo login`). Use a clean checkout of the reviewed tag and rerun package
   verification plus `cargo +1.84.1 publish --locked --dry-run`.
3. Only after explicit release approval, run `cargo +1.84.1 publish --locked`.
   Check the registry version and test installation under an isolated Cargo root.
4. Revoke the bootstrap token, remove any no-longer-needed local credential with
   `cargo logout`, and configure Trusted Publishing for subsequent versions.

## Subsequent Crate Releases

Configure a GitHub environment named **`crates-io`** with required reviewers and
appropriate deployment restrictions. Merely naming the environment in YAML does
not configure its protection. On the crate's Trusted Publishing settings, set:

| Field | Value |
| --- | --- |
| Repository Owner | `vuongvm812` |
| Repository Name | `agent-float-term` |
| Workflow Filename | `crates-io.yml` |
| Environment | `crates-io` |

The manually dispatched **Crates.io** workflow defaults to `dry_run: true` and
does not request publishing credentials in that mode. It resolves the existing
version-matching tag, runs reusable CI, verifies a clean packaged crate, and
performs a publish dry run. There is no tag-push publication trigger.

For an approved publication, set `dry_run: false`, enter exactly `publish TAG`
(for example `publish v0.1.1`), and approve the protected environment. The publish
job repeats verification after approval, rechecks the tag, obtains a short-lived
OIDC token through a commit-pinned `rust-lang/crates-io-auth-action`, and publishes
the selected source. Only that job receives `id-token: write`; no long-lived
registry secret or API-token fallback is configured.

Check registry availability before retrying a failed upload: the upload may have
succeeded even if a later response failed. Do not republish an existing version.
For a broken release, publish a corrected new version and assess whether a yank
is appropriate; yanking neither deletes code nor revokes leaked credentials.

## References

- [Homebrew taps](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
- [Homebrew formula cookbook](https://docs.brew.sh/Formula-Cookbook)
- [Cargo publishing](https://doc.rust-lang.org/cargo/reference/publishing.html)
- [Cargo installation](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
- [Crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing)
