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

The same workflow's `app` matrix job builds the Tauri desktop bundles with the Tauri CLI from
`app/` (no third-party action), only when a release is being made. The version is passed as
`--config '{"version":"X.Y.Z"}'` plus `RUCKCODE_VERSION`, so `tauri.conf.json` and `Cargo.toml`
stay at `0.1.0`. Artifacts: `ruckcode-app-<version>-<target>.{dmg,app.tar.gz}` (macOS aarch64 and
x86_64, the latter cross-built on macos-14), `.msi` and `-setup.exe` (Windows x86_64),
`.AppImage` and `.deb` (Linux x86_64, built on ubuntu-22.04 for glibc 2.35 compatibility), each with
a `.sha256`. `publish` needs `build` and `app`, so any failed bundle blocks the release.

Signing is optional and only active when repository secrets exist: macOS `APPLE_CERTIFICATE`
(base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, plus `APPLE_ID`,
`APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID` for notarization; Windows
`WINDOWS_CERTIFICATE` (base64 .pfx) and `WINDOWS_CERTIFICATE_PASSWORD`. Without them, bundles are
unsigned (Gatekeeper/SmartScreen warnings). To replace the placeholder icons run
`pnpm -C app tauri icon <1024px.png>`.

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
