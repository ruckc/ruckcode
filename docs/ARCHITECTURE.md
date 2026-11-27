# Architecture

ruckcode is split in two layers:

- the **harness** (`ruck-harness-*`): a product-neutral agent engine (LLM providers, tools, sandbox,
  MCP, hooks, the agent loop, config layering, the ACP server);
- the **product** (`ruckcode-*`): the coding agent `rc` and its front-ends, built on the harness.

**The rule: no `ruck-harness-*` crate depends on any `ruckcode-*` crate** (normal, build or dev
dependency). CI enforces it with `scripts/check_harness_boundary.py` (see
[Compatibility guarantees](#compatibility-guarantees-and-the-tests-that-enforce-them)).

## Crate map

| Crate | Layer | What it is |
|-------|-------|------------|
| `ruck-harness-ident` | harness | `AppIdentity`: every product-named string, no dependencies |
| `ruck-harness-llm` | harness | `Provider` trait, Anthropic and OpenAI-compatible clients, mock provider |
| `ruck-harness-sandbox` | harness | Command sandbox: launcher, namespaces, Landlock, seccomp, Seatbelt, allowlist proxy (`rc-sandbox-exec` bin) |
| `ruck-harness-hooks` | harness | Hook config, matching, runner, fix-turn feedback |
| `ruck-harness-tools` | harness | `Tool` trait and its context/output types, sandboxed launcher, plan and ask-the-user tools |
| `ruck-harness-web` | harness | `WebFetch` / `WebSearch` with SSRF protection and the sandbox network gate |
| `ruck-harness-mcp` | harness | MCP client; server tools as `mcp__<server>__<tool>` |
| `ruck-harness-core` | harness | Agent loop, sessions, context and compaction, modes, permissions, transcripts, loop guards, `HarnessConfig` and `ConfigLoader`, extension traits |
| `ruck-harness-acp` | harness | ACP v1 server over the agent, namespaced by the identity |
| `ruckcode-tools` | product | Coding tools: Read, Write, Edit, Glob, Grep, Bash |
| `ruckcode-mise` | product | mise task discovery and execution as tools |
| `ruckcode-agent` | product | The ruckcode `AgentExtension`: prompt, tools, guard hints, config sections, setup, keys, modes API, `init` |
| `ruckcode-cli` | product | `rc`: TUI, headless `-p`, `rc acp`, `rc init`, `rc setup` |
| `ruckcode-app` | product | Tauri v2 desktop shell (separate Cargo workspace and lockfile) |

Dependency graph (an arrow points at the dependency):

```
ruckcode-cli, ruckcode-app
   |-> ruckcode-agent -> ruckcode-tools, ruckcode-mise -+
   |        |                                           |
   |        +--------> ruck-harness-core  <-------------+ (tools/mise use ruck-harness-tools)
   +-> ruck-harness-acp -> ruck-harness-core
                              |-> ruck-harness-{tools, mcp, hooks, llm, sandbox, ident}
ruck-harness-tools -> ruck-harness-{llm, sandbox, ident}
ruck-harness-web   -> ruck-harness-{tools, sandbox, ident}
ruck-harness-mcp   -> ruck-harness-{tools, ident}
```

Every harness crate may depend on other harness crates; nothing in the harness points at the product.

## What belongs where

Harness: anything a different agent (a research assistant, a ticket triager) would also want
unchanged: the loop, providers, permissions, sandboxing, MCP, hooks, transcripts, compaction, retry,
loop guards, the ACP server, generic tools (plan, ask, web).

Product: everything that makes it a *coding* agent or *ruckcode*: the file/search/shell tools, mise,
the system prompt text, the product config sections (`[update]`, `[mise]`), setup and key
management, the mode templates, and the `AppIdentity` value itself.

When a harness crate needs product knowledge, it takes a value (identity) or a trait implementation
(extension) from its caller. It never names a product.

## `AppIdentity`

`ruck_harness_ident::AppIdentity` is a `Copy` struct of `&'static str`s (and one slice), with no
`Default`:

`name`, `env_prefix`, `project_dir`, `config_dir_env`, `user_config_subdir`, `data_dir_name`,
`user_agent`, `homepage_url`, `cli_name`, `acp_namespace`, `extra_protected_project_paths`.

It exists so that no harness crate hardcodes `ruckcode`, `.ruckcode`, `RUCKCODE_*`, `rc` or
`_meta.ruckcode`. The harness derives from it the project directory (`.ruckcode`), the config
and data directories, environment variable names (`identity.env_var("EVENT")`), HTTP user agents,
the paths a sandboxed command cannot write, command names in hints ("run `rc setup`") and the ACP
namespace. The only product value is `ruckcode_agent::identity::RUCKCODE`; it is passed explicitly
(options, constructors, `AgentExtension::identity`), never through a global. The `testing` feature
of the ident crate provides `RUCKCODE` and `ACME` fixtures for tests.

## Extension points (`ruck-harness-core`)

`Agent::new(options, Arc<dyn AgentExtension>)` builds the engine; `ruckcode_agent::extension` adds
`Agent::ruckcode(options)` for the product.

- **`AgentExtension`** (once per process): `identity()`, `prompt()` (a `PromptProvider`),
  `guard_hints()`, `session_policy()` (optional `SessionPolicy`), `mode_schema()`,
  `make_provider(profile, user_dir)`, `session_init(cwd, &HarnessConfig, &toml::Table, &ModeSet)`,
  `decode_plan(tool, input)` and `builtin_tool(name)`. `session_init` receives the merged raw
  config table, so the product reads its own sections from it.
- **`SessionExt`** (once per session, returned by `session_init`): `builtin_tools(mode, cfg,
  user_dir)` (the product's tools for a mode, ahead of `SwitchMode` and MCP tools),
  `dynamic_tools(...)` (runtime-discovered tools such as mise tasks, appended after MCP; default
  none) and `refresh(cwd)` (called on a mode switch).
- **`PromptProvider`**: `base(&PromptCtx)` is the product's opening guidance; `environment_extra` and
  `project_sections` default to empty. The harness assembles the rest (environment, mode, mode list,
  client instructions). `PromptCtx::offers(name)` says whether a built-in tool is available, so the
  prompt never advertises a tool the model cannot call.
- **`GuardHints`**: how the tool-loop guard recognises the product's tools: `edit_tools`,
  `path_keys`, `search_tool` (a search-pattern parser) and `progress_probe` (a `ProgressProbe`, e.g.
  git status). Injected by the extension. `GuardHints::default()` is neutral and disables the
  repeated-search and edit-oscillation detectors; the repeat, no-progress and plan-no-op detectors
  need no hints.
- **`SessionPolicy`**: `apply(cwd, &mut HarnessConfig, &mut ModeSet) -> Vec<String>` adjusts config
  and modes at session start and returns user notices. The harness provides `RemoteSandboxPolicy`
  (project sandbox policy as committed on the remote's default branch); ruckcode uses it.
- **`ModeSchema`**: what a product adds to the mode format. `extra_keys` are the only unknown keys
  copied into `ModeDef::extra`; `check_extra` validates them, `implicit_extra` fills the implicit
  `default` mode, `is_builtin_tool` drives the unknown-tool warning. `ModeSchema::NEUTRAL` adds
  nothing. ruckcode declares `mise_tasks` (typed accessor `RuckcodeModeExt::mise_tasks()`).
- **`Tool` hooks** (in `ruck-harness-tools`, all defaulted): `plan(&Value) -> Option<Vec<PlanItem>>`
  (a tool that maintains the session plan: shown to the client, no-op updates skipped, carried into
  the compaction summary), `auto_allow() -> bool` (runs without a permission prompt when no rule
  applies) and `lean_class() -> LeanClass` (`Keep`, `DropWhenLean`, `OptInWhenLean`: how a profile's
  `lean_tools` treats the tool). The harness has no `TodoWrite` special cases; `decode_plan` lets a
  product replay transcripts containing plan tools the current mode no longer offers.

## Config layering

`ConfigLoader::new(identity).load_table(project_dir, user_dir)` merges, lowest first: built-in
defaults, the user's `config.toml`, the project's `.mcp.json`, the project's
`<project_dir>/config.toml`, then `<ENV_PREFIX>_PROFILE` as `default_profile`. It keeps the
security rules (a project `[sandbox]` is dropped; a project `[web]` keeps only `enabled`).

The result is a raw `toml::Table`. `HarnessConfig::from_table` deserialises and validates the
harness sections (profiles, permissions, MCP, hooks, sandbox, web, rate limit, guards). The product
deserialises its own sections from the same table: ruckcode's `Config` holds a `HarnessConfig` plus
`check_updates`, `[update]` and `[mise]`, and derefs to the harness half. The halves are
deserialised separately (no `serde(flatten)`), which preserves the harness's error messages.

## ACP namespace

`ruck-harness-acp` serves the agent over ACP v1. Extension methods and metadata take their
namespace from `Agent::identity().acp_namespace`: for ruckcode `_ruckcode/*` methods and
`_meta.ruckcode.*`; a product with namespace `acme` gets `_acme/*` and `_meta.acme.*` and nothing
else on the wire. Entry points: `serve(agent, AcpInfo, reader, writer)`, `serve_stdio`,
`spawn_in_process` and `spawn_in_process_raw`. See [ACP.md](ACP.md).

## Building a non-coding agent on the harness

`ruck-harness-core/tests/toy_agent.rs` and `ruck-harness-acp/tests/toy_acp.rs` do exactly this with
no ruckcode crate in the dependency graph. The shape:

```rust
struct AcmePrompt;
#[async_trait]
impl PromptProvider for AcmePrompt {
    async fn base(&self, _: &PromptCtx<'_>) -> String { "You are the ACME assistant.".into() }
}

struct AcmeSession;
#[async_trait]
impl SessionExt for AcmeSession {
    fn builtin_tools(&self, _: &ModeDef, _: &HarnessConfig, _: Option<&Path>) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(Echo)] // any `impl Tool`
    }
    async fn refresh(&mut self, _: &Path) {}
}

struct AcmeExtension;
#[async_trait]
impl AgentExtension for AcmeExtension {
    fn identity(&self) -> &AppIdentity { &ACME }          // your AppIdentity, acp_namespace "acme"
    fn prompt(&self) -> Arc<dyn PromptProvider> { Arc::new(AcmePrompt) }
    fn guard_hints(&self) -> GuardHints { GuardHints::default() }
    fn session_policy(&self) -> Option<Arc<dyn SessionPolicy>> { None }
    fn mode_schema(&self) -> &ModeSchema { &ModeSchema::NEUTRAL }
    fn make_provider(&self, p: &Profile, user_dir: Option<&Path>) -> Result<Arc<dyn Provider>> { /* ... */ }
    async fn session_init(&self, ..) -> Result<Box<dyn SessionExt>> { Ok(Box::new(AcmeSession)) }
    fn decode_plan(&self, _: &str, _: &Value) -> Option<Vec<PlanItem>> { None }
    fn builtin_tool(&self, name: &str) -> Option<Arc<dyn Tool>> { /* ... */ }
}

let agent = Arc::new(Agent::new(AgentOptions::default(), Arc::new(AcmeExtension)));
ruck_harness_acp::serve_stdio(agent, AcpInfo::new(env!("CARGO_PKG_VERSION"))).await?;
```

Tests use `ruck_harness_llm::mock::MockProvider` (scripted replies) as `provider_override`. A real
product returns its provider from `make_provider` (e.g. via the llm crate's Anthropic and OpenAI
clients, which take the identity for the user agent).

## Compatibility guarantees and the tests that enforce them

The refactor was behaviour-preserving: the `rc` CLI, the ACP wire protocol, `config.toml` and
`modes.toml` formats, transcripts and resume, release artifacts, the sandbox launcher
(`__sandbox-exec`, `rc-sandbox-exec`) and every model-visible string are unchanged.

| Guarantee | Test |
|-----------|------|
| ACP wire output is unchanged (157 frames, `_ruckcode/*`, `_meta.ruckcode.*`) | `ruckcode-agent/tests/acp_golden.rs` against `tests/golden/acp-baseline.json`; `acp_agent_server.rs`, `acp_demo_mock.rs` |
| The ACP namespace comes from the identity, nothing else leaks | `ruck-harness-acp/tests/toy_acp.rs` |
| System prompts are byte-identical (19 scenarios) | `ruckcode-agent/tests/prompt_pinning.rs`, `golden/prompts.txt` |
| Config parsing and layering, including `[mise]`, `[update]`, `check_updates` | `ruckcode-agent/tests/config_pinning.rs`, `config_env.rs` |
| Mode parsing, templates, `mise_tasks` round trip | `modes_golden.rs`, `modes_api_pinning.rs`, `golden/modes*.txt` |
| The engine runs with no coding code and a different identity | `ruck-harness-core/tests/toy_agent.rs`, `ruckcode-agent/tests/extension_boundary.rs` |
| Old transcripts (`TodoWrite`) still resume | `ruckcode-agent/tests/tool_hooks.rs` and the transcript tests |
| A different identity changes the strings | unit tests in the sandbox, hooks, llm and mcp crates |
| The harness never depends on the product | `scripts/check_harness_boundary.py` (CI `fmt` job; unit tests in `scripts/test_check_harness_boundary.py`, run by the `site` job) |

The boundary check runs `cargo metadata --no-deps` and fails if any package named `ruck-harness-*`
lists a dependency named `ruckcode-*` of any kind. It needs no network and takes well under a second.

## Known exceptions

A few strings in `ruck-harness-sandbox` still say `rc`, deliberately, because they are byte-identical
compatibility surface rather than configuration:

- the launcher's stderr messages (`rc sandbox: ...`), which the capability probe parses and which
  reach the model through command output;
- the sandbox hostname (`rc-sandbox`), the in-namespace proxy socket (`rc-proxy.sock`), and
  `argv[0]` of the launcher (`rc`);
- the AppArmor hint in the capability report, which names the `rc` binary.

Internal temp-directory prefixes (`rc-sbx-`, `rc-sbx-probe-`) are not user- or model-visible
contracts and are also left as they are. Making any of these identity-derived is possible later
(`AppIdentity::cli_name`) but would change visible output, so it was out of scope for a
behaviour-preserving refactor.
