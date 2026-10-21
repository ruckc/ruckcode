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
| WSL2 (Windows) | The Tauri backend spawns `wsl.exe -d <distro> --cd <linux path> -- rc acp` and relays its stdio the same way. The app lists distros (`wsl.exe -l -v`, falling back to `-l -q`), checks for `rc` inside the distro (and offers to install it from the release archive), and translates `\\wsl$\...` paths to Linux paths. |
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
  `.dmg` and `.app` for both macOS architectures, `.msi` (the only Windows installer; the NSIS installer was dropped: MSI only, which avoids admin UAC prompts), and `.AppImage` and
  `.deb`. They are attached to the same GitHub release with checksums.
- **Code signing and notarization** (macOS, Windows) are optional and need secrets, so they are
  skipped until configured and documented.

## Milestones

1. **Scaffold. Done.** SvelteKit SPA, the Tauri crate, mise tasks, and CI checks. A local host
   that relays ACP, plus a minimal chat with the mock model. See "Milestone 1 notes" below.
2. **Core chat and tools. Done.** Full UI, permissions, AskUserQuestion, modes, model picker and
   sessions. See "Milestone 2 notes" below.
3. **WSL2 host. Done.** Distro discovery, `rc` provisioning inside WSL, path translation, and a
   stdio relay. See "Milestone 3 notes" below.
4. **Setup and keys UI. Done.** Settings dialog with the setup wizard, keys manager and project tab,
   plus the JSON outputs for the CLI subcommands. See "Milestone 4 notes" below.
5. **Transcript browser. Done.** List, search, timeline, metrics, resume and export. See "Milestone 5 notes" below.
6. **Mode/hook editor. Done.** Graphical `modes.toml` editor with comment-preserving saves, plus the new `rc modes` subcommand. See "Milestone 6 notes" below.
7. **Release bundles. Done.** Tauri bundles in the release workflow, app metadata, and docs. See "Milestone 7 notes" below.
8. **Updates. Done.** Signed updater artifacts, GitHub Pages distribution, in-app auto-update, `rc self-update` and a Pages-based WSL `rc` installer. See "Milestone 8 notes" below.

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
  `agent_send` fails; the Rust shell did not yet emit an explicit "agent exited" event (added in milestone 3).
- **Tests.** vitest: reducer, diff, format, projects, markdown, controller (against the fake
  agent), and component tests with `@testing-library/svelte` in jsdom (jsdom pinned to 26
  because 30 needs a newer Node patch than the pinned 22.22.0). Playwright (`@playwright/test`
  1.56, matching the preinstalled chromium-1194) in `app/e2e`, run with `mise run app:e2e` or
  `pnpm -C app e2e` (builds, serves `pnpm preview`, uses `?mock=1`). CI runs it after the build,
  installing chromium with `playwright install --with-deps chromium`.
- **Not done.** Model change to "default" (`_ruckcode/set_model` with null), image/resource
  prompt content, terminal content blocks (shown as a placeholder), slash commands.

## Milestone 3 notes

- **Agent-exit event (carry-over from M2).** `agent_connect` now takes `distro` and an `onEvent`
  `Channel<AgentEvent>` (replacing `onLine`). Events are `{event:"line", line}` and one final
  `{event:"exit", code, message}`, on the same channel so the exit always follows the last line. The
  Rust relay emits it when the agent's output ends: a subprocess/WSL child (code plus the last 20
  stderr lines in `message`), or the in-process agent task finishing or panicking (`code: null`).
  An explicit `agent_disconnect` stays silent. The Tauri transport turns it into `onClose(reason)`
  and `transport.closeReason`, so the lost-connection banner appears at once with the reason
  ("Lost connection to the agent: `rc acp (Ubuntu)` exited with code 3: ...").
- **Backend** (`crates/ruckcode-app/src/wsl.rs`, host kind `wsl` in `host.rs`). Pure, tested on any
  OS: `decode_wsl_output` (UTF-16LE with or without BOM, or UTF-8; strips CR/NUL),
  `parse_distro_list` (`-l -v`; rows are recognised structurally so localized headers and states
  work; only WSL2 distros that are not `docker-desktop*` are selectable), `parse_distro_names`
  (`-l -q` fallback), `parse_rc_check`, `windows_to_linux` / `linux_to_windows`, `agent_args` and
  `build_install_script`. Commands: `platform`, `app_version`, `wsl_list_distros`,
  `wsl_check_rc(distro)`, `wsl_install_rc(distro, onProgress)`, `wsl_translate_path(distro, path)`,
  `wsl_to_windows_path`. Off Windows the `wsl_*` commands return "WSL2 is only available on
  Windows". All `wsl.exe` launches use `CREATE_NO_WINDOW`.
- **Starting the agent:** `wsl.exe -d <distro> --cd <linux cwd> -- sh -lc 'PATH=$HOME/.local/bin:$PATH;
  exec rc acp'` through the same stdio relay as the subprocess host (`spawn_command`, shared). The
  PATH prefix makes a freshly installed `rc` visible to login shells that predate `~/.local/bin`.
  `--` makes `wsl.exe` hand the rest of the command line to the distro's shell, which is why the
  install script is shipped as `echo <base64> | base64 -d | sh` (no quoting or newline problems).
- **Install script.** Detects the arch (`uname -m`: x86_64 gets the musl static build, aarch64 the gnu
  one), downloads `rc-<ver>-<target>.tar.gz` and `.sha256` from the `v<ver>` release (`<ver>` is the
  app version; if that tag has no asset it asks the GitHub API for the latest release), verifies
  the checksum (exit 4 on mismatch), and installs atomically to `~/.local/bin/rc`. Progress is the
  script's `[rc-install] ...` stdout lines, streamed to the UI. The Rust tests run the real script
  against a fake `curl` (good, tampered and fallback cases), so no network is needed.
- **Paths.** `\\wsl$\<d>\...` and `\\wsl.localhost\<d>\...` map to `/...` (distro compared
  case-insensitively; another distro or a network share is rejected with a message),
  `C:\Users\x` maps to `/mnt/c/Users/x`, Linux and `~` paths pass through. For drive paths the
  backend prefers `wslpath -u` inside the distro (custom automount roots) and falls back to the
  pure function. The UI shows "Linux path: ..." under the project field as you type, and the
  project is stored in recents under its translated Linux path. A TypeScript twin
  (`host/wsl-paths.ts`) serves browser mock mode; both sides share the same test cases.
- **UI.** `HostPicker` in the project header: Local | WSL2 (only when `platform()` is `windows`) |
  Custom rc (path to a binary, subprocess host). WSL2 shows the distro dropdown (default, state,
  WSL version; WSL1 and Docker Desktop distros are listed but disabled), Refresh, the `rc` status
  (installed version, "not installed" with Install, or a version-mismatch warning with Update) with
  live install output, and Connect stays disabled until a WSL2 distro has `rc`. A version mismatch
  only warns. Browse opens the native dialog at `\\wsl.localhost\<distro>\` and the result is
  translated. Recents are now objects `{path, host, distro?, rcPath?}` (old string lists migrate)
  and show a host badge; WSL entries are hidden on other platforms.
- **Mock mode.** `?mock=1&platform=windows` pretends to be Windows and swaps in `FakeWsl`:
  Ubuntu-24.04 (default, no rc yet), Debian (older rc 0.0.9), Alpine (WSL1) and docker-desktop
  (not selectable). `mock:exit <code> <stderr>` ends the fake agent with an exit message. Every
  mock connect is recorded in `window.__ruckcodeConnects` for tests.
- **Tests.** Rust: parsers with UTF-16 fixtures, path translation, script builder and execution,
  exit events (code, stderr tail, signal, ordering, silent disconnect) and a stand-in `wsl.exe` that
  checks the argument shape end to end. vitest: Tauri transport (mocked IPC), exit reason in the
  controller, HostPicker, path twin, recents migration. Playwright: `e2e/wsl.spec.ts`.
- **CI.** New `app-windows` job (`windows-2022`): build the frontend, `cargo check --all-targets
  --locked` and `cargo test` for the shell, so the `cfg(windows)` code is compiled for real.
- **Not verifiable without Windows.** Real `wsl.exe` output and quoting, `CREATE_NO_WINDOW`,
  the dialog on `\\wsl.localhost`, and WebView2 behaviour. See the README for a manual checklist.

## Milestone 4 notes

- **Shared backend.** `ruckcode_core::setup_api` (new) holds every JSON shape (camelCase) and the
  logic behind them: `presets`, `state`, `keys_list/add/remove/use`, `PlanInput` -> `SetupPlan`,
  `preview`/`apply`, `list_models` (auth `env` | `keyName` | `secret`), `init_modes` (mise discovery
  included). The `rc` CLI and the Tauri commands both call it, so local and WSL answers are identical.
  Input secrets (`NewKeyInput`, `ModelsAuth::Secret`) are zeroized on drop, redacted in `Debug`, and
  no output type contains one; the plan is wiped after `apply`.
- **Tauri commands** (each has an `allow-*` permission in the capability): `setup_presets`,
  `setup_state`, `list_models`, `setup_preview`, `setup_apply`, `keys_list`, `keys_add`, `keys_remove`,
  `keys_use`, `init_modes`. All but `setup_presets` take `project` and `distro?`. With a distro they run
  `wsl.exe -d D --cd <project> -- sh -lc 'PATH=$HOME/.local/bin:$PATH; exec rc <args>'` (same shape as
  the agent start; `crates/ruckcode-app/src/setup.rs`), read the JSON from stdout (last line starting
  with `{`; a failure uses the `rc: ...` line of stderr), and pass secrets and the plan on stdin only.
  The subprocess host uses the local core (it has no distro).
- **New CLI surface** (see README): `rc setup --json-state`, `--plan-stdin` (whole plan as JSON on
  stdin), `--preview`, `--json` (also with the existing `--non-interactive` flags), `rc models`,
  `rc keys list|add|remove|use --json`, `rc keys add --api-key-stdin`, `rc init --json`.
  Deviation from the plan text: the app uses `--plan-stdin` rather than the flag form because a new
  key's secret and the per-mode choices are easier to pass as one JSON document on stdin.
- **UI.** Settings button in the top bar opens a modal with tabs Providers & models (wizard), API keys
  and Project (arrow keys switch tabs, Esc closes). The wizard is a state machine
  (`state/wizard.svelte.ts`) with the TUI's steps: provider, endpoint, auth (env var with set/unset
  indicator, saved key, new masked key with user/project scope, none), profile (overwrite
  confirmation), models (spinner, retry, manual entry, filter, select all, default), per-mode models
  (dropdowns over every profile's models plus "Profile default"; only changed modes are written; offers
  `rc init` when modes.toml is missing), review (diff) and save, then offers to reconnect the agent.
  The new key's secret is wiped from state once saved (kept on a failed save so it can be retried) and
  the keys form clears its field on submit, success or not.
- **First run.** On Connect the app asks `setup_state`; `needsOnboarding` (no user config and no
  `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`, the TUI's rule) opens the wizard instead of connecting. Closing
  it still connects, so users with another setup are never blocked; "Reconnect the agent now" connects
  (or reconnects, resuming sessions) with the new config.
- **Mock mode.** `FakeSetup` (`host/fake-setup.ts`) implements everything in memory. `?setup=fresh`
  starts with no profile and no modes.toml (first run); `?setupdelay=<ms>` sets the latency. Model lists:
  Anthropic, OpenAI, Ollama and OpenRouter URLs answer; a URL containing `broken` or `fail`, or the
  secret `bad`, returns HTTP 401; an unset env var or unknown key name fails too. It works with
  `&platform=windows` and a WSL host (the distro is passed through as the target).
- **Tests.** Rust: `setup_api` unit tests, `setup.rs` (argv/stdin construction, output parsing, local
  commands, a stand-in `wsl.exe` + fake `rc` proving secrets reach `rc` only on stdin), CLI JSON
  integration tests (`crates/ruckcode-cli/tests/json_cli.rs`, with a tiny local HTTP server for
  `rc models`). vitest: wizard state machine, keys form and dialog. Playwright: `e2e/setup.spec.ts`.
- **Not verifiable without Windows.** The real `wsl.exe` quoting for these calls (same helper shape as
  the agent start).

## Milestone 5 notes

- **Shared backend.** `ruckcode_core::transcript_api` (new, camelCase JSON, same approach as
  `setup_api`): `list(project, query?)`, `read(project, id, {offset, limit})` and
  `search(project, text, limit)`. Transcripts are streamed line by line; one pass computes both the
  summary and the requested page. Summaries are cached in-process by (mtime, size). Unreadable lines
  (truncated tail, corruption, invalid UTF-8) are skipped and counted (`skippedLines`); record indexes
  count readable records only, so paging stays consistent. Ids must match `[A-Za-z0-9_-]{1,128}`, the
  resolved file must sit directly inside `<project>/.ruckcode/sessions/` (symlinks that leave it are
  refused and unlisted), and tests cover `..`, separators, NUL and symlink escapes. Normalized
  records are `{index, ts, type, ...camelCase fields}` with message content as `text`, `thinking`,
  `redacted_thinking`, `tool_use` and `tool_result` blocks (thinking signatures are dropped).
- **CLI.** `rc sessions --json [--query]`, `rc sessions show <id> --json [--offset --limit]`,
  `rc sessions search <text> --json [--limit]` (single-line JSON, errors `rc: msg` and exit 1).
  The plain `rc sessions` text listing still goes through the agent. Tests in `json_cli.rs`.
- **Tauri.** `transcripts_list`, `transcripts_read`, `transcripts_search` (each `project`, `distro?`;
  permissions `allow-transcripts-*`). Local calls run the core on a blocking thread; WSL runs
  `rc sessions ... --json` through the M4 helper (`setup::via_wsl`). The helper's JSON parser now
  accepts arrays (the last stdout line that is an object or array and parses). Rust tests include a
  stand-in `wsl.exe` returning arrays.
- **UI.** A Chat | Transcripts toggle in the top bar (`TranscriptsView`; the plan panel and composer
  are hidden in that view). State lives in `TranscriptsController` (`state/transcripts.svelte.ts`):
  sort/filter (client side), debounced server-side search with stale-answer protection, paged
  detail (200 records; the end sentinel loads more on scroll, plus a Load more button), `jump(index)`
  that loads through a record and focuses it, `loadAll` (used by Metrics and the markdown export).
  `transcripts/timeline.ts` maps records to items (a `tool_call` and its `tool_result` become one card;
  orphans from paging show "result not loaded"), `metrics.ts` computes per-request/turn values,
  `list.ts` sorts and filters. Stored edits are unified diffs, so `DiffView` gained a `patch` prop
  fed by `parseUnifiedDiff`. Charts are inline SVG (request duration bars with TTFT overlay,
  cumulative input and output token lines) using theme variables, no animation, with `role="img"`
  labels and per-request and per-turn tables as the accessible fallback. Rendering is paged rather
  than virtualized. Resume connects first when needed, then opens the session with `session/load`.
- **Mock mode.** `FakeTranscripts` (`host/fake-transcripts.ts`): five seeded sessions (every record
  type; plan; explore; one with 1,996 records; one with a skipped line). `?transcriptdelay=<ms>`.
  The fake agent can load/resume these ids, and `window.__ruckcodeTranscriptCalls` records
  `{op, project, distro}` for WSL tests.
- **Tests.** Rust: core unit tests (summaries, paging, search, corrupt lines, traversal), CLI JSON
  tests, app crate (rc call lines, local commands, stand-in `wsl.exe`). vitest: timeline mapping for
  every record type, markdown export, metrics, list helpers, controller, `TranscriptsView`.
  Playwright: `e2e/transcripts.spec.ts`.
- **Deviations.** Metrics need every record, so opening the Metrics tab loads the rest of a large
  transcript. Search returns one hit per record (the first matching field). Unknown future record
  types count as skipped lines rather than being shown.

## Milestone 6 notes

- **Shared backend.** `ruckcode_core::modes_api` (new, camelCase JSON, same approach as `setup_api`):
  `get` (status `missing|broken|ready`, path, document, `globalHooks`, catalog), `validate`, `preview`
  and `apply`. The catalog has the built-in tools (name, ACP kind, read-only; `SwitchMode` included),
  discovered mise tasks, configured MCP *server* names (tool names need a running server, so the editor
  suggests `mcp__<server>__*` and takes free-text globs), profiles with their models, hook events (with
  whether glob/matcher apply), permission values and the lifecycle templates (the six full ones and
  plan/build/verify). `ModeDto` covers every `ModeDef` field; hooks are a map by event of `HookSpec`s.
  `globalHooks` (the merged `config.toml` `[hooks]`) is read-only: hooks that run in every mode are a
  config concern, and editing `config.toml` safely is the setup wizard's job. Parsing is lenient (dangling
  references still open so they can be fixed); `broken` means TOML syntax or schema errors.
  Only the project file `.ruckcode/modes.toml` is edited, never the user-level one.
- **Comment-preserving writer** (`modes_edit.rs`). The file is split into sections at the explicit
  `[table]`/`[[array]]` headers using `toml_edit` spans (so a `[header]`-looking line inside a
  multi-line prompt cannot split anything); comments directly above a header belong to that section and
  a trailing comment block at the end of the file stays last. Modes are reordered, removed, renamed
  (headers rewritten; `originalId` links old and new id) and added at the text level. Inside a mode only
  keys whose value changed are touched (via `toml_edit` on that section's body, keeping the value's
  trailing comment and the comments above the key); keys set back to a default (`null`, `""`, `[]`, hook
  defaults 2 fix turns and 120 s) are removed; hook entries are matched exactly or by position and
  unchanged ones are reused verbatim. Multi-line prompts are written as `"""` strings. CRLF files stay
  CRLF. A mode whose layout is not the one `rc init` writes (inline tables, dotted keys, interleaved
  tables) is only touched if edited, and then written out fresh (reported in `rewritten`, comments in
  that mode are lost). After rendering, the text is parsed again and compared with the requested
  document; on a mismatch the changed modes are written fresh, and only then does it refuse.
  Validation errors block `preview` and `apply`. `apply` writes atomically (temp file and rename).
- **Validation rules** (paths like `modes[2].hooks.post_edit[0].matcher`): errors for no modes, invalid or
  duplicate ids, a missing default, `next`/`auto_advance_to` naming missing modes, invalid globs
  (mise tasks, MCP tools, permissions, hook globs), bad permission values, empty hook commands, invalid
  matcher regexes (Rust `regex`, anchored like the agent), bad `onFailure` and a zero timeout; warnings
  for unknown tools (the agent ignores them), MCP globs that are not `mcp__...`, self references,
  auto-advance targets missing from a non-empty `next`, an unconfigured profile, a glob on a non-`post_edit`
  hook, a matcher on a non-tool hook and `fix_turn` with 0 fix turns.
- **CLI.** `rc modes get --json`, `rc modes validate --doc-stdin --json`, `rc modes set --doc-stdin
  [--preview] --json` (single-line JSON, errors `rc: msg` exit 1; shapes in the README).
- **Tauri.** `modes_get`, `modes_validate`, `modes_preview`, `modes_apply` (`project`, `distro?`, `doc`;
  permissions `allow-modes-*`). Local runs the core (validation, preview and apply on a blocking thread);
  WSL runs `rc modes ...` through `setup::via_wsl` with the document on stdin. Rust tests include a
  stand-in `wsl.exe` + fake `rc` checking the argument shape and the stdin document. Creating the file
  reuses `init_modes` (full, minimal or a subset of modes).
- **UI.** A third top-bar view, **Modes** (a Settings tab would be too cramped for the list, form and
  diagram). `ModesEditor` (`state/modes.svelte.ts`) holds the draft; components are `ModesView` (list,
  overview, review), `ModeForm`, `HooksEditor`, `PermissionsTable`, `ChipInput`, `FlowDiagram` (layered
  inline SVG; `modes/flow.ts` does the layout), `UnsavedDialog`. Pure helpers (rename with references,
  delete, reorder, templates, client-side validation mirroring the Rust rules, glob previews) live in
  `modes/doc.ts` and `modes/glob.ts`. Client checks show instantly; the host's answer (debounced 250 ms)
  replaces them while it matches the current draft and a failing host call only shows a note. The flow
  is a simple layered drawing (columns by distance from the default mode), edges are curves, auto-advance
  edges are thick and accent coloured, and a text list of transitions is available to screen readers. The
  unsaved-changes guard covers switching view, connecting another project and closing the window
  (`beforeunload`). After saving, the app offers to reconnect the agent (running sessions keep the modes
  they started with).
- **Mock mode.** `FakeModes` (`host/fake-modes.ts`) is the one in-memory `modes.toml` store, shared with
  `FakeSetup` (so the wizard's per-mode models and the Project tab see the same file). Its diffs come
  from a canonical TOML rendering of both documents (the real writer keeps comments and formatting).
  `?modesdelay=<ms>`, `?modes=broken`; works with `&platform=windows` and WSL hosts
  (`window.__ruckcodeModesCalls` records `{op, project, distro}`).
- **Tests.** Rust core: round trips (byte identical for the generated full, minimal and tiny templates,
  CRLF), targeted edits touching only intended lines (prompt, tools, defaults, permissions, hooks),
  add/remove/rename/reorder with trailing comments kept, rename updating references and the default,
  unusual layouts, lookalike headers in prompts, validation rules, get/preview/apply end to end;
  CLI JSON tests; app crate tests with the stand-in `wsl.exe`. vitest: document operations, glob and
  matcher checks, flow layout, `ModesEditor` (validation staleness, review/save, init, broken file) and
  components (chips, permissions, hooks, flow, `ModesView`). Playwright: `e2e/modes.spec.ts`.
- **Deviations / limits.** MCP tool names are not listed (needs running servers). The client-side glob
  and regex checks approximate `globset` and Rust `regex` (the host's answer is authoritative). Comments
  attached to a deleted hook or mode go with it. Reordering modes moves each mode's comments with it,
  and a comment block separated from a header by a blank line stays with the previous mode.

## Milestone 7 notes

- **`app` job in `release.yml`.** Matrix: macOS aarch64 and x86_64 (both on macos-14, the latter
  cross-built via rustup target), Windows x86_64 (windows-2022), Linux x86_64 (ubuntu-22.04: older
  glibc 2.35 means the AppImage and deb run on more distributions; webkit2gtk-4.1 exists there). Uses
  only `actions/*` (checkout, setup-node, cache, upload-artifact) and `pnpm -C app tauri build`.
- **Version.** `--config '{"version":"X.Y.Z"}'` overrides the bundle version at build time and
  `RUCKCODE_VERSION` feeds `app_version` (falls back to `CARGO_PKG_VERSION` locally). Nothing is committed.
- **Artifacts.** `ruckcode-app-<version>-<target>.{dmg,app.tar.gz,msi,AppImage,deb}` plus
  `.sha256`; `publish` waits for `app`, folds them into `SHA256SUMS` and the release. A failed app
  build blocks the release (the CLI and app stay in lockstep, and `rc` install from the app relies on
  matching versions).
- **Signing is optional** (secrets listed in CONTRIBUTING.md): macOS via Tauri's `APPLE_*` env vars
  (exported only if present), Windows by importing the pfx and merging a `certificateThumbprint`
  config. Unsigned otherwise; `.app.tar.gz` is made with `tar` since Tauri only emits it with updater artifacts.
- **Checks.** The deb version, macOS Info.plist version and architecture are verified; GUI binaries are
  never launched, and the cross-built x86_64 macOS app is not run.
- **Metadata.** category DeveloperTool, descriptions, copyright, MIT, homepage, Windows MSI per-user by default (dual-purpose via a vendored WiX template; per-machine via `ALLUSERS=1`),
  macOS minimum 11.0. Icons remain placeholders.
- **Local check.** `tauri build --bundles deb` produced `ruckcode_0.0.0-test_amd64.deb` with the right
  metadata. The macOS and Windows jobs have not run yet; expect first-run fixes (paths under
  `target/<triple>/release/bundle`).

## Milestone 8 notes

Updates, designed around the fact that the repository is private (Release assets are not
publicly downloadable) while GitHub Pages is public.

- **Distribution.** The `pages` job (official `actions/configure-pages`, `upload-pages-artifact`,
  `deploy-pages`, pinned by SHA, environment `github-pages`) deploys the latest release after
  `publish` succeeds: `index.html`, `install.sh`, `latest.json` (Tauri static manifest),
  `rc/latest.json`, and `download/v<version>/*`. Older versions stay on GitHub Releases only. The
  site is built by `scripts/build_site.py` (stdlib Python, tested in `scripts/test_build_site.py`);
  `scripts/check_site_limits.sh` fails the run above 900 MB total or 100 MB per file. A manual run of
  the Release workflow with `redeploy_pages` rebuilds the site from the release assets.
- **Signing.** One minisign key pair from `tauri signer generate`. Secrets
  `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`, variable `RUCKCODE_UPDATER_PUBKEY`. App jobs add
  `bundle.createUpdaterArtifacts` and `plugins.updater.pubkey` through `tauri build --config` only
  when both exist; the `site` job signs the `rc` archives with `tauri signer sign --app-version`
  and re-verifies every `.sig` with `minisign` against the public key. Without them nothing
  changes except that no updater artifacts or manifests are produced and the apps say "updates not
  configured". The committed `tauri.conf.json` has an empty `pubkey` and `requireSignedVersion`.
- **Desktop app.** `tauri-plugin-updater` is registered but not exposed to the webview; the app
  has four narrow commands (`update_check`, `update_download` with a progress `Channel`,
  `update_install`, `update_restart`; capability grants only these). Restart uses `AppHandle::restart`,
  so `tauri-plugin-process` is not needed. `manual_reason` turns off self-update for deb, rpm
  and unbundled builds (the MSI updates itself). `UpdateController` (`state/updates.svelte.ts`) checks at startup and every
  6 hours, downloads automatically (setting `ruckcode.autoUpdate` in localStorage), shows a subtle
  progress line and then the "Update vX ready" banner; restart is held back while any session runs.
  Skipped in dev and plain mock mode; `?mock=1&update=1|manual|unconfigured|uptodate|error|
  faildownload` fakes it. Tests: vitest (controller, banner, settings tab) and Playwright
  `e2e/updates.spec.ts`.
- **`rc`.** `rc self-update [--check] [--yes] [--version-only] [--insecure-skip-signature]`
  (`crates/ruckcode-cli/src/update.rs`): manifest, semver comparison, download, SHA-256, minisign
  signature (`minisign-verify`; the signed trusted comment must carry `version:<manifest version>`
  to stop rollback replays), extraction and `self_replace`. The key comes from
  `RUCKCODE_UPDATER_PUBKEY` at build time (`build.rs`). Daily non-blocking hint cached in
  `<cache>/ruckcode/update-check.json`; `RUCKCODE_NO_UPDATE_CHECK`, `check_updates` in
  `config.toml`; never in `rc acp`. Tests use a local HTTP server and a throwaway key pair.
- **WSL.** `wsl.rs` now fills the shared `site/install.sh` (also served as `install.sh`) with the
  site URL and public key and runs it in the distro; it reads the latest version from
  `rc/latest.json` (falling back to the app's own version when no manifest exists).
- **Limits.** The Windows updater runs the MSI installer (the process exits;
  passive mode, no UAC for per-user installs; an update of a per-machine install needs elevation). macOS and Windows builds are still not code-signed, so first launch warnings stay.
  The release steps for macOS and Windows updater artifacts have not run on CI yet; expect
  first-run path fixes like in milestone 7.

## v1 complete

All seven milestones are done: chat, tools and permissions, WSL2 host, setup and keys, transcript
browser, mode/hook editor, and installable bundles.

## Open items for later

- Mobile and remote access (`rc serve`, ACP over WebSocket with token auth).
- Auto-update is done (milestone 8); remaining: a one-time key setup by the maintainer, deb/rpm
  self-update (Tauri supports them, we only point to the download page), delta updates.
- Configure code-signing secrets (Apple, Windows) so releases stop being flagged; replace placeholder icons.
- Linux aarch64 and ARM Windows bundles.
- Branch protection ruleset still needs applying (see CONTRIBUTING.md).

## Decision: UI toolkit (2026-10-21)

**Stay on Tauri v2 until Tauri v3 is stable, then re-evaluate.**

- **Mobile (iOS and Android) is a hard requirement**, even though it is out of scope for v1.
  Whatever toolkit we use must support it without a later rewrite. Tauri v2 does, and the
  SvelteKit UI and ACP client carry over unchanged. The mobile app is a remote ACP client of
  `rc serve`.
- **Known issue:** Linux pulls `glib` 0.18 through Tauri's GTK 3 stack (RUSTSEC-2024-0429).
  It cannot be upgraded, because gtk-rs no longer publishes GTK 3 bindings and
  webkit2gtk/wry/tao require `glib ^0.18`. The unsound `VariantStrIter` is never called. The
  issue is ignored with this justification in `crates/ruckcode-app/.cargo/audit.toml` and
  gated by the CI `audit` job. Revisit it on every Tauri or wry upgrade.
- **Alternatives considered:**
  - **gpui:** no mobile support.
  - **gpui-toolkit:** third-party mobile backends maintained by one person, distributed as
    source beta, with mobile unproven in a shipped app.
  - **Slint:** a full rewrite, and iOS support is young.
  - **Dioxus:** uses the same wry/WebKitGTK stack.
  - **Electron:** desktop only.
  - **Capacitor or PWA:** viable mobile shells for the SvelteKit UI if Tauri mobile
    disappoints.
- **When Tauri v3 ships**, check whether it leaves GTK 3 and upgrade if it does.
