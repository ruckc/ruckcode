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

1. **Scaffold. Done.** SvelteKit SPA, the Tauri crate, mise tasks, and CI checks. A local host
   that relays ACP, plus a minimal chat with the mock model. See "Milestone 1 notes" below.
2. **Core chat and tools. Done.** Full UI, permissions, AskUserQuestion, modes, model picker and
   sessions. See "Milestone 2 notes" below.
3. **WSL2 host.** Distro discovery, `rc` provisioning inside WSL, path translation, and a stdio
   relay.
4. **Setup and keys UI**, including the JSON outputs for the CLI subcommands.
5. **Transcript browser.**
6. **Mode/hook editor**, including the new `rc modes` subcommand.
7. **Release bundles** in CI, and documentation.

## Milestone 1 notes

Built as planned, with these deviations and details:

- **Separate Cargo workspace.** `cargo ... --workspace` ignores `default-members`, so the only
  way to keep `--workspace` webkit-free was to exclude `crates/ruckcode-app` from the root
  workspace (`exclude` in the root `Cargo.toml`) and give it its own `[workspace]` and
  `Cargo.lock`. Use `--manifest-path crates/ruckcode-app/Cargo.toml` (CI does) instead of
  `-p ruckcode-app`. It depends on `ruckcode-core` and `ruckcode-acp` by path.
- **SvelteKit 3.** Configuration lives in `vite.config.ts` (no `svelte.config.js`), and
  `$lib` is gone, so imports are relative. TypeScript is 6.x (svelte-check and typescript-eslint
  do not support 7 yet). The ACP SDK is `@agentclientprotocol/sdk` 2.0 (stable v1 entrypoint,
  `client().onRequest(...)`/`onNotification(...)` with `ndJsonStream`; it needs `zod` as a peer).
- **Transport.** `LineTransport { send, onLine, close? }` is adapted to the SDK's `Stream` with
  `ndJsonStream` over Web Streams (`app/src/lib/acp/transport.ts`). `_ruckcode/ask_user_question`
  is registered as a custom request handler with a zod-validated payload. The client advertises
  `session.notices` and `_meta.ruckcode.{askUserQuestion,notices}`.
- **Backend.** `agent_connect(cwd, host?, rcPath?, onLine: Channel<String>) -> id`,
  `agent_send(id, line)`, `agent_disconnect(id)`. Hosts: `local` (embedded agent through the new
  `ruckcode_acp::spawn_in_process_raw`) and `subprocess` (`rc acp`, path configurable; no UI for
  it yet). Capabilities grant only the three app commands (generated `allow-*` permissions via
  the build script's app manifest) and `dialog:allow-open`.
- **Browser demo.** `?mock=1` swaps the Tauri host for a scripted in-memory agent
  (`host/fake-agent.ts`, also used by the vitest suite and for screenshots).
- **Superseded by milestone 2:** Playwright, project history, session list, markdown and diff views.

## Milestone 2 notes

- **Layout.** Left sidebar (project header with folder input, Browse in Tauri, recent projects in
  `localStorage` under `ruckcode.recent`, always inside try/catch; session list from
  `session/list`, New session, close), main chat, and a collapsible plan panel on the right.
  Below 900px the side panels become overlays.
- **State.** `ChatController` (`state/chat.svelte.ts`) owns one connection and a map of open
  sessions (`SessionView`: transcript model, `running`, `loading`, prompt `queue`).
  `session/update` is routed by `sessionId`, so a session keeps streaming while another is shown.
  Opening a stored session uses `session/load` (replay, `canLoad` from `loadSession`; falls back
  to `session/resume`); already-open sessions just switch. Reconnect after a crash uses
  `session/resume` for every open session and keeps their transcripts on screen. Permission and
  AskUserQuestion requests queue connection-wide and show one at a time (`1 of N pending`).
  Prompts typed during a turn queue like the TUI and run in order; Esc cancels (queue is kept,
  as in the TUI).
- **Reducer** (`state/reducer.ts`) covers every update kind: chunks, tool calls (rawInput,
  locations, text output, `diff` contents), plan, mode, usage, notices, `config_option_update`
  (select options flattened) and `session_info_update`. Turn ends are pushed by the controller from
  the `session/prompt` stop reason.
- **Rendering.** `marked` + DOMPurify (links get `target=_blank rel=noopener`), copy buttons added
  to code blocks after each streamed render; diffs computed with the `diff` package
  (`structuredPatch`, unified or side-by-side); tool kind icons are inline SVG.
- **Dialogs.** Permission: y/a/n (allow once / always / reject), Esc cancels the turn; `switch_mode`
  proposals get explicit wording ("Switch to X" / "Stay in current mode"). Ask: one tab per
  question, 1-9 pick options, Alt+Left/Right switch tabs, Ctrl+Enter submits, "(Recommended)"
  in a label becomes a badge, free-text Other. Both trap focus and restore it on close.
- **Fake agent** (`host/fake-agent.ts`) now implements initialize capabilities, `session/new|list|
  load|resume|close|set_mode|set_config_option`, a shared `FakeStore` (so mock reconnect finds
  sessions) and prompt scripts: `mock:write`, `mock:ask`, `mock:askmulti`, `mock:mode <id>`,
  `mock:edit`, `mock:bash`, `mock:md`, `mock:notice`, `mock:slow`, `mock:error`, `mock:crash`.
  `?mock=1&delay=<ms>` sets the streaming delay.
- **Connection loss.** `LineTransport` gained an optional `onClose`; when it fires (or the SDK
  connection closes) the app shows a Reconnect banner. The Tauri transport raises it when
  `agent_send` fails; the Rust shell does not yet emit an explicit "agent exited" event, so a
  silently dead agent is only noticed on the next send.
- **Tests.** vitest: reducer, diff, format, projects, markdown, controller (against the fake
  agent), and component tests with `@testing-library/svelte` in jsdom (jsdom pinned to 26
  because 30 needs a newer Node patch than the pinned 22.22.0). Playwright (`@playwright/test`
  1.56, matching the preinstalled chromium-1194) in `app/e2e`, run with `mise run app:e2e` or
  `pnpm -C app e2e` (builds, serves `pnpm preview`, uses `?mock=1`). CI runs it after the build,
  installing chromium with `playwright install --with-deps chromium`.
- **Not done.** Model change to "default" (`_ruckcode/set_model` with null), image/resource
  prompt content, terminal content blocks (shown as a placeholder), slash commands.

## Open items for later

- Mobile and remote access (`rc serve`, ACP over WebSocket with token auth).
- Auto-update (Tauri updater; needs signing keys).
- Branch protection ruleset still needs applying (see CONTRIBUTING.md).
