# ACP support

`ruckcode-acp` serves the agent over the Agent Client Protocol (v1, newline-delimited JSON-RPC 2.0).
`rc acp` serves on stdio; `rc` itself talks to an in-process agent over a duplex pipe.

## Methods (client -> agent)

| Method | Notes |
| --- | --- |
| `initialize` | Negotiates `protocolVersion` 1. `loadSession: true`, `embeddedContext: true`, no image/audio, no auth methods. |
| `authenticate` | No-op, returns `{}`. |
| `session/new` | `{cwd}` -> `{sessionId, modes}`. |
| `session/load` | Replays history as `session/update` notifications, then returns `{modes}`. |
| `session/prompt` | Returns `{stopReason}` (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`). |
| `session/cancel` | Notification. |
| `session/set_mode` | Emits `current_mode_update`. |

Unknown methods (including unknown `_`-prefixed ones) fail with `-32601`.

## Agent -> client

- `session/update`: `user_message_chunk` (replay), `agent_message_chunk`, `agent_thought_chunk`,
  `tool_call`, `tool_call_update` (text content plus one `diff` per edited file), `plan`,
  `current_mode_update`. Notices are sent as `agent_message_chunk` text `"\n> {notice}\n"`.
- `session/request_permission`: options `allow_once` ("Allow"), `allow_always` ("Always allow",
  only when offered), `reject_once` ("Reject"). A cancelled outcome or RPC error cancels the turn.

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

### `_ruckcode/list_sessions` (client -> agent)

`{"cwd": "/project"}` -> `{"sessions": [{"id", "createdAt", "updatedAt", "firstPrompt", "mode"}]}`,
newest first. Used by `rc --resume`.

## Modes

Lifecycle modes (from `.ruckcode/modes.toml`) map to ACP session modes: `id`/`name`/`description`
per mode, `currentModeId` is the active mode. The agent may switch modes itself (`SwitchMode` tool);
that also arrives as `current_mode_update`.
