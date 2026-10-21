# ruckcode (`rc`)

ruckcode is a Claude Code-style coding agent written in Rust. Instead of permission modes it has
**lifecycle modes** (explore, design, implement, test, review, release), it exposes your
**mise tasks as tools**, runs **hooks** (for example format/lint after every edit, with automatic
fix turns), and keeps a complete, committable **transcript** of every session in your repository.

The agent speaks only the [Agent Client Protocol](docs/ACP.md) (ACP). The `rc` terminal UI and the
headless `rc -p` mode are ACP clients talking to an in-process agent, and `rc acp` serves the same
agent to editors such as Zed. See [docs/PLAN.md](docs/PLAN.md) for the design.

## Install

```sh
cargo install --path crates/ruckcode-cli   # installs the `rc` binary
# or, with mise:
mise run install
```

## Quick start

Configuration lives in `~/.config/ruckcode/config.toml` (user) and `.ruckcode/config.toml`
(project, overrides user per key). The built-in defaults define two profiles, `anthropic`
(default; reads `ANTHROPIC_API_KEY`) and `openai` (reads `OPENAI_API_KEY`), so for Anthropic you
only need (or just run `rc` and follow the setup wizard, see [Setup](#setup)):

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cd your-project
rc                       # interactive TUI
rc "explain this repo"   # TUI with an initial prompt
```

Any OpenAI-compatible server works as a profile, for example Ollama:

```toml
# ~/.config/ruckcode/config.toml
default_profile = "ollama"

[profiles.ollama]
provider = "openai"                  # "anthropic" | "openai"
model = "qwen2.5-coder:14b"
base_url = "http://localhost:11434/v1"
# api_key_env = "OPENAI_API_KEY"     # optional for openai-provider profiles
# max_tokens = 32000                 # default 32000
# context_window = 200000            # default 200000
# thinking = "high"                  # passed through: Anthropic `thinking` object / OpenAI reasoning_effort
```

Other top-level keys: `compaction_threshold` (default 0.8), `max_turn_requests` (default 100),
`[mise] lazy_threshold` (default 12), `[permissions]` (tool name or glob such as `"mise__*"` to
`allow` / `ask` / `deny`), and global `[[hooks.<event>]]` tables. `RUCKCODE_PROFILE` overrides
`default_profile`; `--profile` and `--model` override per run.

## Setup

The first time you run interactive `rc` with no user config and no `ANTHROPIC_API_KEY` /
`OPENAI_API_KEY` (and no project `.ruckcode/config.toml`), a setup wizard starts automatically.
Run it again any time with `rc setup` or the `/setup` slash command. Steps: provider (Anthropic,
OpenAI, Ollama, OpenRouter, or any OpenAI-compatible URL such as vLLM / LM Studio), endpoint,
authentication, profile name, models, models per mode, and a review of exactly what will be
written. The wizard fetches the endpoint's model list (Space toggles, `a` selects all, typing
filters, `/` focuses the filter; if the fetch fails you can retry or type ids by hand). `Esc` goes
back a step, `Ctrl+C` aborts without writing anything. `-p` and `rc acp` never start the wizard; with
no usable key they fail with an error that points at `rc setup`.

What gets written, and where:

- `~/.config/ruckcode/config.toml`: the `[profiles.<name>]` entry (provider, default `model`,
  `models` available to `/model`, `base_url`, `api_key_env` or `api_key_name`) and `default_profile`.
  Existing comments and formatting are preserved. Set `RUCKCODE_CONFIG_DIR` to use another directory.
- `~/.config/ruckcode/credentials.toml` (mode 0600): API keys, only if you choose to save one.
  Keys are never written to `config.toml`, the project or the repository.
- `.ruckcode/modes.toml` (the project, shared with your team): `profile` and `model` for each mode
  you assigned a model to. "Profile default" removes both keys. If the project has no `modes.toml`
  the wizard offers to run `rc init` first.

Scripted setup:

```sh
rc setup --non-interactive --provider anthropic --api-key-env ANTHROPIC_API_KEY \
  --models claude-opus-5-5,claude-haiku-5-5 --mode-model explore=claude-haiku-5-5
rc setup --non-interactive --provider ollama --models qwen2.5-coder:14b
echo "$KEY" | rc setup --non-interactive --provider openrouter --api-key-name work --api-key-stdin \
  --models anthropic/claude-sonnet-5 --profile openrouter
```

Flags: `--provider anthropic|openai|ollama|openrouter`, `--base-url`, `--profile` (default: the
provider), `--models a,b` (required), `--default-model`, `--api-key-env NAME`, `--api-key-name NAME`
(+ `--api-key-stdin` to store the secret), `--project-key` (see below), `--no-default`, and repeated
`--mode-model MODE=MODEL` (`MODE=default` clears; the model belongs to the profile being set up).

`/model` lists the profile's `models` and switches the model for the current session (stored in the
transcript as a `model_change` record; `/model default` clears it).

### Named API keys

A profile finds its key in this order: `api_key_name` (a name in `credentials.toml`; a missing name
is an error telling you to run `rc keys add NAME`), then the env var named by `api_key_env`.
Names are references, not secrets, so they are safe to commit; profiles can share a key, and a
project can pick its own:

```toml
# ~/.config/ruckcode/credentials.toml (0600), managed by `rc keys` / the wizard
[keys.personal]
api_key = "sk-ant-..."
description = "my key"

# .ruckcode/config.toml (committed): only this field is overridden, the rest of the user's
# [profiles.anthropic] (model, max_tokens, ...) still applies.
[profiles.anthropic]
api_key_name = "client-acme"
```

Profiles from user and project config are merged field by field, so a project config can override
just `api_key_name` (or `model`, ...) of a user profile. The wizard's auth step offers: an env var,
an existing named key, a new named key (name + masked secret), or no key, and whether to save the
reference in your profile or in this project's config.

```sh
rc keys list                       # names, masked last 4 chars, descriptions, who uses them
rc keys add NAME [--description D] # secret from stdin or a hidden prompt
rc keys remove NAME
rc keys use NAME [--profile P] [--project]   # set api_key_name in user (or project) config
```

Team note: `modes.toml` references profile names. Teammates need profiles with the same names
(run `rc setup` or copy the `[profiles.*]` block). If a mode's `profile` is not configured,
`rc` warns at the start of the session ("mode `test` uses profile `openrouter`, which isn't
configured") and falls back to the default profile and its model instead of failing.

## Modes

Without configuration there is a single implicit `default` mode with all tools. Generate lifecycle
modes for a project:

```sh
rc init                         # writes .ruckcode/modes.toml: explore, design, implement, test, review, release
rc init --minimal               # plan, build, verify
rc init --modes explore,implement,test   # a subset
rc init --diff                  # diff the existing file against current templates
rc init --force                 # overwrite an existing modes.toml
```

`rc init` fills each mode's `mise_tasks` from your repository's real `mise tasks ls`. Modes are
read from `.ruckcode/modes.toml` and `~/.config/ruckcode/modes.toml`. Switch modes in the TUI with
**Shift+Tab** (cycles) or `/mode <id>`, or start in one with `rc --mode <id>`. The agent can also
propose a switch with the `SwitchMode` tool; the user is asked unless the target is in
`auto_advance_to`.

Each mode can pick its own model or profile:

```toml
[modes.implement]
description = "Write code for the approved plan"
profile = "anthropic"          # optional: profile for this mode
model = "claude-sonnet-5-5"    # optional: model id (beats --model and the profile)
prompt = "Implement the approved plan. Keep changes minimal."
tools = ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "AskUserQuestion", "SwitchMode", "TodoWrite"]
mise_tasks = ["build", "gen:*"]
next = ["test", "review"]
auto_advance_to = []
permissions = { Bash = "ask", Edit = "allow", Write = "allow" }
```

## mise tasks as tools

`rc` discovers tasks with `mise tasks ls --json` and exposes those matching the current mode's
`mise_tasks` selectors as tools named `mise__<task>` (argument: `args: string[]`), run via
`mise run <task> -- <args>` with truncated output streamed back as tool-call updates. When more than
`[mise] lazy_threshold` tasks match (default 12), the model gets two meta-tools, `MiseListTasks` and
`MiseRun`, instead, so task schemas are loaded only on demand.

## MCP servers

`rc` is an MCP client (built on the official [`rmcp`](https://crates.io/crates/rmcp) SDK; stdio and
streamable-HTTP transports, no OpenSSL). Servers come from, in increasing priority: user
`~/.config/ruckcode/config.toml`, the project's `.mcp.json` (Claude Code format), and the project's
`.ruckcode/config.toml`. Entries merge by server name; a later definition replaces the whole entry.

```toml
# config.toml
[mcp]
lazy_threshold = 20            # default

[mcp_servers.github]           # stdio server
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "..." }
# cwd = "tools"                # optional, relative to the project

[mcp_servers.docs]             # streamable HTTP server
url = "https://example.com/mcp"
headers = { Authorization = "Bearer ..." }
```

```json
// .mcp.json
{"mcpServers": {
  "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"GITHUB_TOKEN": "..."}},
  "docs": {"type": "http", "url": "https://example.com/mcp", "headers": {"Authorization": "Bearer ..."}}
}}
```

Servers are started on the first prompt of a session, concurrently with a 30s timeout each. A server
that fails to start is reported as a notice and recorded in the transcript (`mcp_servers` record), and
never blocks the others or the session.

Each tool is exposed as `mcp__<server>__<tool>` (sanitized to `[A-Za-z0-9_-]`, at most 64 chars).
**Modes opt in** with glob selectors over those full names; a mode without `mcp_tools` gets no MCP
tools (the implicit `default` mode gets `["mcp__*"]`):

```toml
[modes.implement]
mcp_tools = ["mcp__github__*", "mcp__docs__search"]
```

If a mode selects more than `[mcp] lazy_threshold` tools, the model gets `McpListTools` (optional
`server`) and `McpCall` (`tool`, `arguments`) instead of one tool per MCP tool.

**Permissions**: MCP tools ask by default, except tools whose `readOnlyHint` annotation is true, which
are allowed. Override with the usual globs in `[permissions]` (config or mode), e.g.
`"mcp__github__*" = "allow"` or `"mcp__github__delete*" = "deny"`. Results are text (non-text items
are described, e.g. `[image: image/png]`) and truncated to 30,000 bytes.

`rc mcp list` connects to the configured servers and prints each server's status and tool names.

Over ACP, `session/new` and `session/load` `mcpServers` (stdio and `http`) are added to that session
and override same-named configured servers; see [docs/ACP.md](docs/ACP.md). Child processes are killed
when the session ends.

## Hooks

Events: `session_start`, `user_prompt_submit`, `pre_tool_use`, `post_tool_use`, `post_edit`,
`mode_change`, `stop`. Hooks are shell commands (run via `sh -c` in the project root) that receive
JSON on stdin. They can be global (`[[hooks.post_edit]]` in config.toml) or per mode. `on_failure` is
`ignore` (default), `block` (report to the user) or `fix_turn` (feed the output back to the model
as a new turn, up to `max_fix_turns`, default 2). Other keys: `glob` (post_edit), `matcher`
(anchored regex on tool name for pre/post_tool_use), `timeout_secs` (default 120).
`post_edit` hooks run once per turn over the de-duplicated edited files (`{files}` placeholder).

```toml
[[modes.implement.hooks.post_edit]]
glob = "**/*.rs"
run = "mise run fmt"
on_failure = "fix_turn"
max_fix_turns = 2
```

## AskUserQuestion

The model can ask structured questions (options, multi-select, an implicit "Other") with the
`AskUserQuestion` tool. ACP clients that advertise `_meta.ruckcode.askUserQuestion` get the
`_ruckcode/ask_user_question` extension request; other clients get one `session/request_permission`
per question. See [docs/ACP.md](docs/ACP.md).

## Trying rc without an API key

A profile with `provider = "mock"` uses a deterministic demo model that reacts to the last
message, so you can exercise the TUI's question, permission, todo and streaming UI offline:

```toml
# .ruckcode/config.toml
default_profile = "demo"
[profiles.demo]
provider = "mock"
model = "mock"
```

| Prompt | What the model does |
|---|---|
| `mock:ask` (or "ask me") | `AskUserQuestion`: one single-select, one multi-select |
| `mock:write <path> <text>` | `Write` (asks permission) |
| `mock:edit <path> <old> => <new>` | `Edit` |
| `mock:bash <cmd>` | `Bash` |
| `mock:todo` | `TodoWrite` with 3 items |
| `mock:mode <id>` | `SwitchMode` |
| `mock:think` | a thinking block plus text |
| `mock:tool <Name> <json>` | any tool call |
| anything else | lists these commands and echoes your prompt |

Tools are only called when the current mode offers them. Tool results are echoed back as text
("You chose: ...", "Tool `X` returned: ...", "Tool failed: ..."). Set `RUCKCODE_MOCK_DELAY_MS`
to slow the streamed chunks down. The `mock:` prefix avoids clashing with TUI `/` commands.

## Sessions and transcripts

Every session is written to `.ruckcode/sessions/<YYYYMMDD-HHMMSS-xxxxxx>.jsonl` inside the project,
intended to be **committed with the code it produced** for traceability. The append-only JSONL
transcript holds full prompts, assistant messages including thinking blocks, every tool call (input,
output, permission decision, edits as diffs, duration), hook runs, mode changes, compactions, and
per-request metrics (model, usage, latency, time to first token, tokens/sec).

```sh
rc sessions              # list this project's sessions
rc -c                    # continue the most recent session
rc -r                    # pick a session interactively
rc -r 20250102-030405-a1b2c3   # resume by id (also --resume=<ID>)
rc -r "fix the bug"      # not an id: treated as the prompt, opens the picker
```

## Headless mode

```sh
rc -p "summarise the failing tests"
git diff | rc -p "review this diff"            # stdin is appended to the prompt
rc -p --allow Bash,Edit "fix the lint errors"  # auto-allow these tools
rc -p --yolo "do the thing"                    # allow every permission request
rc -p --output-format stream-json "..."        # raw session/update notifications, one per line
```

Other flags: `--mode`, `--profile`, `--model`, `--cwd`, `-c`, `-r <ID>`. Without `--allow`/`--yolo`
permission requests are denied; AskUserQuestion is cancelled. Exit codes: `0` finished (end of turn),
`130` cancelled (Ctrl+C), `1` any error or other stop reason.

## Editor integration (ACP)

`rc acp` serves the agent over newline-delimited JSON-RPC on stdio. In Zed:

```json
{
  "agent_servers": {
    "ruckcode": {
      "command": "rc",
      "args": ["acp"]
    }
  }
}
```

Wire types come from the official [`agent-client-protocol-schema`](https://crates.io/crates/agent-client-protocol-schema)
crate (ACP v1, stable surface only) and the JSON-RPC transport from the official
[`agent-client-protocol`](https://crates.io/crates/agent-client-protocol) SDK. Supported methods (including `session/list`, `session/resume`,
`session/close` and the `model` config option), notifications and the `_ruckcode/*` extensions are
documented in [docs/ACP.md](docs/ACP.md).

## Desktop app (preview)

A Tauri v2 + SvelteKit desktop app that is another ACP client, like the TUI and editors; see
[docs/APP_PLAN.md](docs/APP_PLAN.md). It is a scaffold for now: pick a project folder, connect,
and chat (streaming text, tool calls, permission and AskUserQuestion dialogs, cancel, current
mode). Try it with the mock model (see above) before configuring a real provider.

```sh
mise run app:install   # pnpm install in app/ (node 22, pnpm 10 are pinned in mise.toml)
mise run app:dev       # Vite dev server + Tauri window (embedded agent)
mise run app:check     # svelte-check, eslint, prettier --check
mise run app:test      # vitest
mise run app:build     # release bundles (slow)
```

On Linux, building the Tauri shell needs webkit2gtk (`libwebkit2gtk-4.1-dev libsoup-3.0-dev
libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev build-essential`).
`crates/ruckcode-app` is its own Cargo workspace, so `cargo build/test/clippy --workspace` for
`rc` never needs webkit. Build it with
`cargo check --manifest-path crates/ruckcode-app/Cargo.toml` (or `mise run app:lint:rust`,
`app:test:rust`).

The frontend can also run in a plain browser against a scripted fake agent:
`pnpm -C app dev` then open `http://localhost:5173/?mock=1`.

## Workspace layout

```
crates/
  ruckcode-core     agent loop, sessions, context assembly, modes, permissions, config
  ruckcode-llm      Provider trait + Anthropic and OpenAI-compatible clients
  ruckcode-tools    Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion, TodoWrite, SwitchMode
  ruckcode-mise     mise task discovery, schema mapping, execution
  ruckcode-mcp      MCP client (rmcp): server connections, tools as `mcp__<server>__<tool>`
  ruckcode-hooks    hook config, matching, runner, fix-turn feedback
  ruckcode-acp      ACP agent server + client plumbing (official ACP SDK for JSON-RPC)
  ruckcode-cli      `rc` binary: TUI, headless `-p`, `rc acp`, `rc init`
  ruckcode-app      Tauri v2 desktop shell (separate Cargo workspace, see below)
app/                SvelteKit UI for the desktop app (TypeScript, Svelte 5, pnpm)
```

## Development

```sh
mise run test    # cargo test --workspace
mise run lint    # cargo clippy --workspace --all-targets -- -D warnings
mise run fmt     # cargo fmt --all
# the desktop app is excluded from --workspace; see "Desktop app" for its tasks
```

Set `RUCKCODE_LOG=debug` for logs (the TUI logs to `~/.cache/ruckcode/rc.log`). Provider tests run
against a local fake SSE server and need no API keys.

## License

MIT — see [LICENSE](LICENSE).

## Releases

Every push to `main` is examined by [git-cliff](https://git-cliff.org) (config
in `cliff.toml`). When it finds release-worthy Conventional Commits since the
last `v*` tag (`feat` = minor, `fix`/`perf`/`refactor` = patch, breaking = major,
or minor while below 1.0.0), the release workflow builds `rc` and publishes a
GitHub Release `vX.Y.Z` with:

- `rc-<version>-<target>.tar.gz` (`.zip` on Windows) containing `rc`, `LICENSE`, `README.md`
- per-archive `.sha256` files and a combined `SHA256SUMS`
- `CHANGELOG.md` and release notes generated from the commits

Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`x86_64-unknown-linux-musl` (static), `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-pc-windows-msvc`. The first release is `v0.1.0`.
Commits containing only `docs`/`ci`/`chore`/`test`/`style`/`build`/`revert` do not
trigger a release. The version is embedded at build time (`RUCKCODE_VERSION`), so
`Cargo.toml` is never bumped and no changelog commit lands on `main`; local
builds report `<pkg version>-dev+<sha>`. See `CONTRIBUTING.md` for the commit convention.
