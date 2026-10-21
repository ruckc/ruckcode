# ruckcode desktop app: plan

A Tauri v2 + SvelteKit desktop app that sits alongside the `rc` CLI. Mobile is out of scope
for now, but the UI should stay responsive so it remains possible later.

## Decisions

- **Platforms:** macOS, Windows and Linux desktop. **WSL2 on Windows is supported.**
- **The ACP client lives in TypeScript and uses the official ACP TS SDK.** The app is just
  another ACP client, like the TUI and editors, so modes, mise tasks, hooks, MCP,
  AskUserQuestion and transcripts work unchanged.
- **JS tooling:** node and pnpm, pinned in `mise.toml`, with mise tasks for dev, build and check.
- **v1 scope:** core chat and tools, the setup and keys UI, the transcript browser, and the
  mode/hook editor.

## Architecture

```
app/                      SvelteKit SPA (adapter-static, TypeScript, Svelte 5)
crates/ruckcode-app/      Tauri v2 shell (Rust)
```

Agent connection ("agent host"), chosen per window and project:

| Host | How |
|------|-----|
| Local (default) | The Tauri backend embeds `ruckcode_core::Agent` behind `ruckcode_acp` over an in-process pipe. ACP JSON-RPC lines are relayed to the webview through a Tauri `Channel`, and outgoing lines go back through a Tauri command. |
| WSL2 (Windows) | The Tauri backend spawns `wsl.exe -d <distro> --cd <linux path> -- rc acp` and relays its stdio the same way. The app lists distros (`wsl.exe -l -q`), checks for `rc` inside the distro (and offers to install it from the release archive), and translates `\\wsl$\...` paths to Linux paths. |
| Subprocess (any OS, optional) | `rc acp` as a child process. Useful for running against a specific `rc` binary. |

The frontend has one transport interface (`send(line)` and `onLine(cb)`) for all three hosts,
with the official ACP TS SDK on top. Our `_ruckcode/*` extensions (ask_user_question,
set_model, list_sessions) become typed helpers. The client advertises `askUserQuestion` and
`notices` capabilities.

Non-ACP features (config, keys, modes.toml, transcripts) are Tauri commands backed by
`ruckcode-core` (`setup`, `keys`, `modes`, `init`, `transcript` modules). When the host is
WSL2, the same features run through `rc` subcommands inside the distro and need JSON output
modes: `rc sessions --json`, `rc keys list --json`, `rc setup --non-interactive`, and
`rc modes get/set --json` (new).

## v1 features

1. **Core chat and tools.**
   - Project picker (recent projects; on Windows, a WSL distro selector) and a sessions sidebar
     using `session/list`, resume and close.
   - Streaming chat with collapsible reasoning, and tool cards with status, output and
     side-by-side diffs.
   - Plan/todo panel, token usage, and notices.
   - Permission dialog (allow once, always, or reject) and AskUserQuestion dialog
     (single-select, multi-select, Other).
   - Mode switcher, with confirmation when the agent proposes a switch, and a model picker
     using the `model` config option.
   - Cancel button.
2. **Setup and keys UI.** A graphical version of `rc setup`: provider and endpoint, auth
   (env var, named key or new key), profile, live model list, per-mode models, diff review and
   save. Plus a named-keys manager (add, remove, use; secrets never shown).
3. **Transcript browser.** Browse and search `.ruckcode/sessions/*.jsonl` and view a timeline
   of messages, reasoning, tool calls with diffs, hook runs and mode changes, with per-request
   metrics (latency, time to first token, tokens/s, usage). It is read-only, and a session can
   be resumed from it.
4. **Mode/hook editor.** Edit `.ruckcode/modes.toml` graphically: tools, mise tasks (from
   discovery), MCP tool selectors, permissions, model and profile, `next` and
   `auto_advance_to`, and hooks per event (with glob, matcher, on_failure and fix turns). It
   saves with `toml_edit` so comments are preserved, shows a diff before writing, and offers
   `rc init` for projects without the file.

## Workspace and tooling

- `mise.toml` pins node and pnpm, with tasks `app:dev`, `app:build`, `app:check` (svelte-check
  and eslint) and `app:test` (vitest, plus Playwright against `pnpm preview` with a mock agent).
- The Rust workspace gains `crates/ruckcode-app`, with Tauri behind its own crate so `rc` builds
  don't need webview dependencies. The mock profile (`provider = "mock"`) drives UI tests and
  demos.

## CI and releases

- **CI:** a new `app` job runs pnpm install (cached by `pnpm-lock.yaml` through
  `actions/cache`), `app:check`, `app:test` and `cargo check -p ruckcode-app` on Linux, with
  webkit2gtk installed via apt. Only official `actions/*` actions are used, consistent with the
  existing workflows.
- **Release:** builds the Tauri bundles with the Tauri CLI directly (no third-party action):
  `.dmg` and `.app` for both macOS architectures, `.msi` and NSIS `.exe`, and `.AppImage` and
  `.deb`. They are attached to the same GitHub release with checksums.
- **Code signing and notarization** (macOS, Windows) are optional and need secrets, so they are
  skipped until configured and documented.

## Milestones

1. **Scaffold.** SvelteKit SPA, the Tauri crate, mise tasks, and CI checks. A local host that
   relays ACP, plus a minimal chat with the mock model.
2. **Core chat and tools.** Full UI, permissions, AskUserQuestion, modes, model picker and
   sessions.
3. **WSL2 host.** Distro discovery, `rc` provisioning inside WSL, path translation, and a stdio
   relay.
4. **Setup and keys UI**, including the JSON outputs for the CLI subcommands.
5. **Transcript browser.**
6. **Mode/hook editor**, including the new `rc modes` subcommand.
7. **Release bundles** in CI, and documentation.

## Open items for later

- Mobile and remote access (`rc serve`, ACP over WebSocket with token auth).
- Auto-update (Tauri updater; needs signing keys).
- Branch protection ruleset still needs applying (see CONTRIBUTING.md).
