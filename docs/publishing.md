# Publishing

Release archives, the Homebrew tap, and crates.io are separate publication steps.
They must identify the same reviewed source version. Workflows do not create source
commits or tags, publish a stable GitHub draft automatically, or push to the tap.
This guide does not authorize publication by an automation agent.

Current state: release preparation exists locally, but no release, installable
tap formula, or crate has been published. See the [release checklist](release-checklist.md)
for acceptance gates and [installation](installation.md) for user-facing commands.

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
   Review the tap diff and commit/publish it as the maintainer. No workflow writes
   cross-repository commits or uses a tap write token.
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

Trusted Publishing currently requires an already-published crate, so the first
publication is a separate maintainer action:

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
