# ACP support

`ruckcode-acp` serves the agent over the Agent Client Protocol (v1, newline-delimited JSON-RPC 2.0).
`rc acp` serves on stdio; `rc` itself talks to an in-process agent over a duplex pipe.

Wire types are the official [`agent-client-protocol-schema`](https://crates.io/crates/agent-client-protocol-schema)
crate, pinned to `=1.12.0` with `default-features = false` (no `schemars`) and **no `unstable_*`
features**. `ruckcode_acp::schema` re-exports its `v1` types plus our extension types and helpers.
The JSON-RPC transport (`rpc.rs`) and the request dispatch are our own.

Inbound parsing is lenient where the official types are strict, so lenient clients keep working:
a missing `protocolVersion` means v1; `mcpServers` may be omitted (and a malformed or unsupported
entry is skipped, not fatal); prompt content blocks of unknown type are skipped (logged at debug)
instead of failing the prompt.

## Capabilities advertised by `initialize`

```json
{"protocolVersion": 1,
 "agentCapabilities": {
   "loadSession": true,
   "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
   "mcpCapabilities": {"http": true, "sse": false},
   "sessionCapabilities": {"list": {}, "resume": {}, "close": {}}},
 "authMethods": [], "agentInfo": {"name": "ruckcode", "version": "..."}}
```

Client capabilities we look at: `session.notices` (standard `notice` updates, see below),
`_meta.ruckcode.askUserQuestion` and `_meta.ruckcode.notices` (extensions). `rc` sets all of them.

## Methods (client -> agent)

| Method | Notes |
| --- | --- |
| `initialize` | Negotiates `protocolVersion` 1 (`min(requested, 1)`). |
| `authenticate` | No-op, returns `{}` (no auth methods are advertised). |
| `session/new` | `{cwd, mcpServers?}` -> `{sessionId, modes, configOptions}`. See [MCP servers](#mcp-servers). |
| `session/load` | `{sessionId, cwd, mcpServers?}`. Replays history as `session/update` notifications, then returns `{modes, configOptions}`. |
| `session/resume` | Same as `session/load` but without replaying history. |
| `session/list` | `{cwd?, cursor?}` -> `{sessions: [{sessionId, cwd, title, updatedAt, _meta}]}`, newest first, never paginated. `title` is the first line of the first prompt; `_meta.ruckcode` has `createdAt`, `mode`, `firstPrompt`. Sessions are stored per project, so a missing `cwd` lists the agent process's working directory. |
| `session/close` | Cancels any running prompt and drops the session's in-memory state (MCP connections included). The transcript stays on disk; `session/load`/`resume` can reopen it. |
| `session/prompt` | Returns `{stopReason}` (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`). |
| `session/cancel` | Notification. |
| `session/set_mode` | Emits `current_mode_update`. |
| `session/set_config_option` | Only the `model` option: `{sessionId, configId: "model", value: "<model id>"}` -> `{configOptions}`. Pins the model for the session like `_ruckcode/set_model`. |

Not implemented: `logout`, `session/delete`, `session/fork`, `fs/*` and `terminal/*` (the agent never
calls the client for files), `elicitation/*`. Unknown methods (including unknown `_`-prefixed ones)
fail with `-32601`.

### Config options

`session/new`, `session/load` and `session/resume` return `configOptions` with one select:

```json
{"id": "model", "name": "Model", "category": "model", "type": "select",
 "currentValue": "claude-opus-5", "options": [{"value": "claude-opus-5", "name": "claude-opus-5"}]}
```

`options` are the session profile's `model` plus its `models`; `currentValue` is the effective model
for the current mode. Whenever the effective model changes (pin, or a mode switch that changes the
model) the agent sends a `config_option_update` with the new state.

## Agent -> client

- `session/update`: `user_message_chunk` (replay), `agent_message_chunk`, `agent_thought_chunk`,
  `tool_call`, `tool_call_update` (text content plus one `diff` per edited file; `content` is
  omitted when there is nothing new, because a present `content` replaces what the client shows),
  `plan`, `current_mode_update`, `config_option_update`, `usage_update`, `notice`.
  - `usage_update`: `{used, size}` after every model request, where `used` is the provider-reported
    input plus output tokens and `size` the profile's `context_window`. Not sent when the provider
    reports no usage.
  - Notices (hook output, model changes, MCP start-up failures, ...) are sent as the standard
    `notice` update (`severity: "info"`, `title`) only if the client advertised
    `clientCapabilities.session.notices` (or `_meta.ruckcode.notices = true`). Otherwise they stay
    `agent_message_chunk` text `"\n> {notice}\n"`.
- `session/request_permission`: options `allow_once` ("Allow"), `allow_always` ("Always allow",
  only when offered), `reject_once` ("Reject"). A cancelled outcome or RPC error cancels the turn.

Wire notes (the official serializer's conventions): `tool_call` omits `status` when `pending` and
`kind` when `other`; diffs of new files omit `oldText`.

## Extensions

### `_ruckcode/ask_user_question` (agent -> client)

Used only if the client sets `clientCapabilities._meta.ruckcode.askUserQuestion = true` in `initialize`.

```json
{"sessionId": "...", "questions": [{"question": "Which DB?", "header": "DB", "multiSelect": false,
  "options": [{"label": "sqlite", "description": ""}]}]}
```
Result: `{"answers": [{"question": "Which DB?", "selected": ["sqlite"], "other": null}], "cancelled": false}`.
`cancelled: true` fails the tool call.

Without the capability, each question is asked via `session/request_permission`, with one
`allow_once` option per choice (`optionId` `opt-<index>`, `name` the label); free text is unavailable.

### `_ruckcode/list_sessions` (client -> agent) - deprecated

Deprecated alias of the standard `session/list`, kept for existing clients.
`{"cwd": "/project"}` -> `{"sessions": [{"id", "createdAt", "updatedAt", "firstPrompt", "mode"}]}`,
newest first. `rc` itself uses `session/list`.

### `_ruckcode/set_model` (client -> agent)

`{"sessionId": "...", "model": "claude-haiku-5-5"}` -> `{}`. Pins the model for the session,
overriding the mode's and profile's model (recorded in the transcript as a `model_change` record and
announced as a notice). `model: null` or absent clears the pin, which the standard
`session/set_config_option` cannot express; used by the TUI's `/model`. Selecting a model with
`session/set_config_option` is equivalent to a non-null pin.

## MCP servers

`mcpServers` on `session/new`, `session/load` and `session/resume` are added to that session
(overriding same-named servers from config) and connect on the first prompt:

```json
[{"name": "fs", "command": "/usr/bin/mcp-fs", "args": ["--root", "."], "env": [{"name": "K", "value": "v"}]},
 {"type": "http", "name": "docs", "url": "https://example.com/mcp", "headers": [{"name": "Authorization", "value": "Bearer t"}]}]
```

SSE entries are ignored (`mcpCapabilities.sse` is false). Which tools a mode sees is still governed by
its `mcp_tools` selectors (the implicit default mode selects all). Start-up failures arrive as a
notice. MCP tool calls appear as ordinary `tool_call`s titled
`<server>: <tool>` with kind `read` (read-only annotated) or `other`, and ask via
`session/request_permission` unless allowed by `[permissions]`.

## Modes

Lifecycle modes (from `.ruckcode/modes.toml`) map to ACP session modes: `id`/`name`/`description`
per mode, `currentModeId` is the active mode. The agent may switch modes itself (`SwitchMode` tool);
that also arrives as `current_mode_update`.

## Compatibility with the previous hand-written schema

Intended wire differences, all covered by `crates/ruckcode-acp/tests/golden.rs` (baseline captured
before the migration): `sessionCapabilities`/`auth` in the `initialize` response; `configOptions` in
session responses; `tool_call_update` without `content` instead of `content: []`; diffs without
`"oldText": null`; `tool_call` without default `status`/`kind`; new `usage_update`,
`config_option_update` and (for capable clients) `notice` updates.
