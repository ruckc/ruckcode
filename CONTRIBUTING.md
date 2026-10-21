# Contributing

## Commit messages: Conventional Commits

Releases are fully automatic and derived from commit messages, so every commit
(and every PR title, which becomes the squash-merge commit) must follow
[Conventional Commits](https://www.conventionalcommits.org/):

    <type>(<optional scope>)<!>: <description>

| Type | Effect on release |
|------|-------------------|
| `feat` | minor bump |
| `fix`, `perf`, `refactor` | patch bump |
| `feat!:` / `fix!:` / a `BREAKING CHANGE:` footer | major bump (minor while version < 1.0.0) |
| `docs`, `style`, `test`, `build`, `ci`, `chore`, `revert` | no release |

If a push to `main` contains only non-releasing types since the last `v*` tag,
no tag or release is created. The `Conventional Commits` workflow checks the PR
title and each commit subject on every pull request; prefer squash-merging so
the (validated) PR title is the commit on `main`.

## Releases

See the "Releases" section of the README. Maintainers never bump `Cargo.toml`
or edit a changelog by hand: the version is injected at build time through
`RUCKCODE_VERSION`, and `CHANGELOG.md` is attached to each GitHub Release
rather than committed.

The same workflow's `site` job signs the `rc` archives, builds the Pages site and checks its size,
`publish` creates the GitHub Release (it needs `build`, `app` and `site`), and only then `pages`
deploys. The same workflow's `app` matrix job builds the Tauri desktop bundles with the Tauri CLI from
`app/` (no third-party action), only when a release is being made. The version is passed as
`--config '{"version":"X.Y.Z"}'` plus `RUCKCODE_VERSION`, so `tauri.conf.json` and `Cargo.toml`
stay at `0.1.0`. Artifacts: `ruckcode-app-<version>-<target>.{dmg,app.tar.gz}` (macOS aarch64 and
x86_64, the latter cross-built on macos-14), `.msi` and `-setup.exe` (Windows x86_64),
`.AppImage` and `.deb` (Linux x86_64, built on ubuntu-22.04 for glibc 2.35 compatibility), each with
a `.sha256` (plus a `.sig` for the updater bundles when signing is configured). `publish` needs
`build`, `app` and `site`, so any failed bundle blocks the release.

Signing is optional and only active when repository secrets exist: macOS `APPLE_CERTIFICATE`
(base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, plus `APPLE_ID`,
`APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID` for notarization; Windows
`WINDOWS_CERTIFICATE` (base64 .pfx) and `WINDOWS_CERTIFICATE_PASSWORD`. Without them, bundles are
unsigned (Gatekeeper/SmartScreen warnings). To replace the placeholder icons run
`pnpm -C app tauri icon <1024px.png>`.

## Publishing and updates (one-time setup)

The repository is **private** but its GitHub Pages site is **public**. Release assets on a private
repository cannot be downloaded without credentials, so the release workflow deploys the *latest*
release to Pages (`https://ruckc.github.io/ruckcode/`), and the desktop updater, `rc self-update`
and `install.sh` all fetch from there. Everything on Pages is public: do not put anything private
into a release. Older versions stay only on the private GitHub Releases.

One-time steps (repository admin):

1. **Pages source.** Settings, Pages, Build and deployment, Source: **GitHub Actions**. (The
   `pages` job uses the official `actions/configure-pages`, `upload-pages-artifact` and
   `deploy-pages` actions and the `github-pages` environment; if you restrict that environment to
   branches, allow `main`.)
2. **Generate the updater signing key** on your machine (never commit it, never paste it into an
   issue):

   ```sh
   pnpm -C app tauri signer generate -w ~/.tauri/ruckcode.key
   ```

   Choose a non-empty password. This writes `~/.tauri/ruckcode.key` (private, encrypted) and
   `~/.tauri/ruckcode.key.pub` (public).
3. **Secrets** (Settings, Secrets and variables, Actions, *Secrets*):
   `TAURI_SIGNING_PRIVATE_KEY` = the contents of `~/.tauri/ruckcode.key`,
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = its password.
4. **Variable** (same page, *Variables*): `RUCKCODE_UPDATER_PUBKEY` = the contents of
   `~/.tauri/ruckcode.key.pub` (one line of base64). It is public; it is baked into the app
   (merged into `tauri.conf.json` with `--config` at build time and compiled in) and into `rc`
   (`crates/ruckcode-cli/build.rs` reads the same environment variable).
5. Back up the key and password somewhere safe. If they are lost, installed copies can never
   update again and users must reinstall from the download page.

Behaviour without signing configured (a fork, or before the steps above): everything is built as
before, but no updater artifacts, `.sig` files or manifests are produced, `latest.json` and
`rc/latest.json` are **not** published, and the apps report "updates not configured" instead of
failing. The download page and `install.sh` (checksum only) are still deployed. With the secret
set but the variable missing (or the reverse) the workflow treats signing as not configured and
prints a warning.

The release workflow verifies every `.sig` with `minisign` against `RUCKCODE_UPDATER_PUBKEY`
before publishing, so a key pair that does not match fails the release instead of silently
shipping updates nobody can install.

**Key rotation.** Installed apps and `rc` only trust the public key they were built with, so a new
key cannot be introduced through an update signed with the new key. To rotate: ship a release
signed with the *old* key whose binaries embed the *new* public key (set the variable to the new
key but keep the old private key in the secrets for that one release), then switch the secrets to
the new private key for the following release. If the old key is lost or compromised, users must
reinstall manually from the download page. Never reuse the key for anything else.

**Pages limits.** The site must stay under 1 GB with files under 100 MB. The release workflow
fails with a clear message otherwise (`scripts/check_site_limits.sh`, limits 900 MB / 100 MB).
The Linux AppImage (about 90 MB) is the largest file and is needed for Linux updates; if it ever
approaches the limit, slim the bundle (strip, fewer bundled libraries) rather than dropping it.
Because only the latest version is hosted, the size does not grow release over release.

**Redeploying Pages.** Run the *Release* workflow manually (Actions, Release, Run workflow) with
`redeploy_pages` ticked: it re-downloads the latest release's assets with `gh release download`,
rebuilds the site and deploys it, without building or releasing anything. The *Edit release notes*
workflow does this automatically when it edits the latest release. For local checks of the site
tooling run `python3 -m unittest scripts/test_build_site.py`; `scripts/build_site.py` builds a
site from a directory of artifacts.

**Testing against another site.** `RUCKCODE_UPDATE_BASE_URL` (runtime, must be https for the
desktop app) overrides `https://ruckc.github.io/ruckcode` for `rc self-update`, `install.sh` and
the desktop app. The default is a constant in `crates/ruckcode-cli/src/update.rs` and
`crates/ruckcode-app/src/update.rs` (a test keeps them, the workflow and `tauri.conf.json` in sync).

Platform notes: the desktop app updates itself on macOS, from the Windows NSIS installer and from
the Linux AppImage. `.deb` and `.msi` installs only get an "update available, download from the
download page" notice. `rc self-update` refuses for package-manager installs. Tests never touch a
real key: unit and integration tests generate a throwaway minisign key pair in memory.

## CI notes

- Every GitHub Action is pinned to a full commit SHA with a `# vX.Y.Z` comment;
  Dependabot (weekly) proposes updates. Keep that format when adding actions.
- Workflows use only official GitHub `actions/*` actions, pinned by commit SHA.
  Rust is installed with the runner's preinstalled `rustup`; git-cliff is
  downloaded from its release and checked against a pinned SHA-512.
- Caching uses `actions/cache` (restore/save) keyed by job, OS, rustc version and
  `Cargo.lock`, covering the cargo registry, git deps and `target/`. Caches are
  saved only on `main` (even when a job fails); PRs restore but never write, so
  they cannot thrash the repository cache budget.
- Tests use plain `cargo test` (no extra tool install).
- The `app` job covers the desktop app: pnpm (via corepack, version pinned in the workflow and
  `app/package.json`) with the store cached through `actions/cache` keyed on
  `app/pnpm-lock.yaml`, `app:check`, vitest, the frontend build, then webkit2gtk from apt and
  `cargo check` of `crates/ruckcode-app` (a separate Cargo workspace with its own `Cargo.lock`,
  so the other jobs' `--workspace` commands never need webkit).
- The `site` job runs the Python tests of `scripts/build_site.py` and `site/install.sh`.
- Recommended branch protection for `main`: require the `fmt`, `clippy`, `test`,
  `check`, `PR title` and `PR commits` checks, and allow squash merging.

## Branch protection

`main` is protected by the ruleset in [`.github/rulesets/main.json`](.github/rulesets/main.json):

- Changes land through pull requests only, merged by **squash** (the PR title becomes the
  Conventional Commit on `main`). No approvals are required.
- These checks must pass on an up-to-date branch: `fmt`, `clippy`, `test`,
  `check (macos-14)`, `check (windows-2022)`, `PR title` and `PR commits`.
- Review threads must be resolved. Force pushes, branch deletion and non-linear history are
  blocked.
- Repository admins can bypass the rules only when merging a pull request, for example to land
  a fix for broken CI. They cannot push directly.

The release workflow is unaffected because it creates tags and releases through the API.

To apply or update the ruleset (needs repo admin):

```sh
# create
gh api -X POST repos/ruckc/ruckcode/rulesets --input .github/rulesets/main.json
# update an existing one
gh api repos/ruckc/ruckcode/rulesets --jq '.[] | select(.name=="main") | .id'
gh api -X PUT repos/ruckc/ruckcode/rulesets/<id> --input .github/rulesets/main.json
```

Or import it in the GitHub UI: Settings → Rules → Rulesets → New ruleset → Import a ruleset.
