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
only need:

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

Supported methods, notifications and the `_ruckcode/*` extensions are documented in
[docs/ACP.md](docs/ACP.md).

## Workspace layout

```
crates/
  ruckcode-core     agent loop, sessions, context assembly, modes, permissions, config
  ruckcode-llm      Provider trait + Anthropic and OpenAI-compatible clients
  ruckcode-tools    Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion, TodoWrite, SwitchMode
  ruckcode-mise     mise task discovery, schema mapping, execution
  ruckcode-hooks    hook config, matching, runner, fix-turn feedback
  ruckcode-acp      ACP JSON-RPC server and client plumbing
  ruckcode-cli      `rc` binary: TUI, headless `-p`, `rc acp`, `rc init`
```

## Development

```sh
mise run test    # cargo test --workspace
mise run lint    # cargo clippy --workspace --all-targets -- -D warnings
mise run fmt     # cargo fmt --all
```

Set `RUCKCODE_LOG=debug` for logs (the TUI logs to `~/.cache/ruckcode/rc.log`). Provider tests run
against a local fake SSE server and need no API keys.

## License

MIT — see [LICENSE](LICENSE).
