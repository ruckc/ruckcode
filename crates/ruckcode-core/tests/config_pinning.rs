//! Pins the behaviour of `config.toml` loading (parsing, layering, security rules, validation,
//! unknown keys) so that splitting `Config` into a generic and a product part cannot change it.
//!
//! Nothing here touches the process environment; see `config_env.rs` for that.

use ruck_harness_sandbox::{NamespaceMode, NetworkMode, ProjectPolicy};
use ruckcode_core::config::Config;
use ruckcode_core::{Permission, ProviderKind};
use std::path::Path;

/// Every section of `config.toml`, with non-default values.
const FULL: &str = r#"
default_profile = "local"
compaction_threshold = 0.65
max_turn_requests = 42
check_updates = false

[profiles.local]
provider = "openai"
model = "gemma4:26b"
models = ["gemma4:26b", "qwen3"]
api_key_name = "mykey"
api_key_env = "MY_ENV_KEY"
base_url = "http://localhost:11434/v1"
max_tokens = 8000
context_window = 64000
thinking = { type = "enabled", budget_tokens = 1024 }
temperature = 0.4
top_p = 0.9
tools = ["Read", "Bash"]
lean_tools = true

[profiles.local.pricing]
input = 1.0
output = 2.0
cache_read = 0.1

[profiles.local.model_pricing."qwen3"]
input = 3.0
output = 4.0

[profiles.local.thinking_guard]
budget_tokens = 8000
max_nudges = 1

[profiles.local.context]
compact_threshold = 0.5
max_tool_result_tokens = 500

[profiles.local.tool_loop_guard]
enabled = false
repeat_count = 7

[profiles.local.ollama]
repeat_penalty = 1.15
repeat_last_n = 64

[mcp]
lazy_threshold = 7

[mcp_servers.fs]
command = "mcp-fs"
args = ["--root", "/tmp"]
env = { K = "v" }

[mcp_servers.remote]
type = "http"
url = "http://example.test/mcp"
headers = { Authorization = "Bearer x" }

[permissions]
Bash = "ask"
Write = "deny"
"mise__*" = "allow"

[[hooks.post_edit]]
run = "mise run fmt"
glob = "*.rs"
on_failure = "fix_turn"
max_fix_turns = 3
timeout_secs = 99

[[hooks.stop]]
run = "echo done"

[rate_limit]
max_auto_wait_secs = 5
max_retries = 2
pacing = false

[thinking_guard]
enabled = false
repeat_count = 6
window_chars = 1234
max_nudges = 3

[tool_loop_guard]
repeat_count = 6
plan_noop_count = 5
window = 12
search_repeat_count = 9
oscillation_enabled = false
max_nudges = 4

[context]
compact_threshold = 0.7
max_tool_result_tokens = 100
max_tool_result_fraction = 0.25
output_reserve_tokens = 2000

[sandbox]
enabled = false
project_policy = "off"
strict = true
namespaces = "required"
network = "full"
allowed_domains = ["a.test"]
extra_write_paths = ["/w"]
extra_read_paths = ["/r"]
deny_read_paths = ["/d"]
cache_paths = ["/c"]
allowed_ports = [8080]
mask_env_files = false
private_tmp = false
create_cache_dirs = false

[web]
enabled = false
allow_private = true

[web.search]
provider = "searxng"
url = "https://searx.example"

[mise]
lazy_threshold = 33

[update]
auto = false
check = false
"#;

fn load_str(text: &str) -> anyhow::Result<Config> {
    Config::from_toml_str(text)
}

fn err(text: &str) -> String {
    format!("{:#}", load_str(text).unwrap_err())
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn every_section_of_a_user_config_parses() {
    let c = load_str(FULL).unwrap();
    assert_eq!(c.default_profile, "local");
    assert_eq!(c.compaction_threshold, 0.65);
    assert_eq!(c.max_turn_requests, 42);
    assert!(!c.check_updates);

    // Profiles: the built-in ones are still there next to the user's.
    assert_eq!(
        c.profiles.keys().collect::<Vec<_>>(),
        ["anthropic", "local", "openai"]
    );
    let p = &c.profiles["local"];
    assert_eq!(p.name, "local");
    assert_eq!(p.provider, ProviderKind::Openai);
    assert_eq!(p.model, "gemma4:26b");
    assert_eq!(p.models, ["gemma4:26b", "qwen3"]);
    assert_eq!(p.api_key_name.as_deref(), Some("mykey"));
    assert_eq!(p.api_key_env.as_deref(), Some("MY_ENV_KEY"));
    assert_eq!(p.base_url.as_deref(), Some("http://localhost:11434/v1"));
    assert_eq!((p.max_tokens, p.context_window), (8000, 64000));
    assert_eq!(p.thinking.as_ref().unwrap()["budget_tokens"], 1024);
    assert_eq!((p.temperature, p.top_p), (Some(0.4), Some(0.9)));
    assert_eq!(p.tools.as_deref().unwrap(), ["Read", "Bash"]);
    assert!(p.lean_tools);
    let pr = p.pricing.unwrap();
    assert_eq!((pr.input, pr.output, pr.cache_read), (1.0, 2.0, Some(0.1)));
    assert_eq!(pr.cache_write, None);
    assert_eq!(p.model_pricing["qwen3"].input, 3.0);
    assert_eq!(p.pricing_for("qwen3").unwrap().output, 4.0);
    let tg = p.thinking_guard.as_ref().unwrap();
    assert_eq!((tg.budget_tokens, tg.max_nudges), (Some(8000), Some(1)));
    let cx = p.context.as_ref().unwrap();
    assert_eq!(
        (cx.compact_threshold, cx.max_tool_result_tokens),
        (Some(0.5), Some(500))
    );
    let tl = p.tool_loop_guard.as_ref().unwrap();
    assert_eq!((tl.enabled, tl.repeat_count), (Some(false), Some(7)));
    let ol = p.ollama.as_ref().unwrap();
    assert_eq!(
        (ol.repeat_penalty, ol.repeat_last_n),
        (Some(1.15), Some(64))
    );
    assert_eq!(ol.frequency_penalty, None);
    // The built-in profiles keep their defaults.
    assert_eq!(c.profiles["anthropic"].model, "claude-opus-5-5");
    assert_eq!(c.profiles["anthropic"].max_tokens, 32000);
    assert_eq!(c.profiles["openai"].model, "gpt-5");

    assert_eq!(c.mcp.lazy_threshold, 7);
    assert_eq!(c.mcp_servers.keys().collect::<Vec<_>>(), ["fs", "remote"]);
    assert_eq!(c.mcp_servers["fs"].command.as_deref(), Some("mcp-fs"));
    assert_eq!(c.mcp_servers["fs"].args, ["--root", "/tmp"]);
    assert_eq!(c.mcp_servers["fs"].env["K"], "v");
    assert_eq!(c.mcp_servers["remote"].kind.as_deref(), Some("http"));
    assert_eq!(c.mcp_servers["remote"].headers["Authorization"], "Bearer x");

    assert_eq!(c.permissions["Bash"], Permission::Ask);
    assert_eq!(c.permissions["Write"], Permission::Deny);
    assert_eq!(c.permissions["mise__*"], Permission::Allow);

    assert_eq!(c.hooks.post_edit.len(), 1);
    let h = &c.hooks.post_edit[0];
    assert_eq!(h.run, "mise run fmt");
    assert_eq!(h.glob.as_deref(), Some("*.rs"));
    assert_eq!((h.max_fix_turns, h.timeout_secs), (3, 99));
    assert_eq!(c.hooks.stop[0].run, "echo done");
    assert!(c.hooks.session_start.is_empty());

    assert_eq!(c.rate_limit.max_auto_wait_secs, 5);
    assert_eq!(c.rate_limit.max_retries, 2);
    assert!(!c.rate_limit.pacing);

    assert!(!c.thinking_guard.enabled);
    assert_eq!(c.thinking_guard.repeat_count, 6);
    assert_eq!(c.thinking_guard.window_chars, 1234);
    assert_eq!(c.thinking_guard.max_nudges, 3);

    assert_eq!(c.tool_loop_guard.repeat_count, 6);
    assert_eq!(c.tool_loop_guard.plan_noop_count, 5);
    assert_eq!(c.tool_loop_guard.window, 12);
    assert_eq!(c.tool_loop_guard.search_repeat_count, 9);
    assert!(!c.tool_loop_guard.oscillation_enabled);
    assert_eq!(c.tool_loop_guard.max_nudges, 4);

    assert_eq!(c.context.compact_threshold, Some(0.7));
    assert_eq!(c.context.max_tool_result_tokens, 100);
    assert_eq!(c.context.max_tool_result_fraction, 0.25);
    assert_eq!(c.context.output_reserve_tokens, Some(2000));

    let s = &c.sandbox;
    assert!(!s.enabled && s.strict && !s.mask_env_files && !s.private_tmp);
    assert!(!s.create_cache_dirs);
    assert_eq!(s.project_policy, ProjectPolicy::Off);
    assert_eq!(s.namespaces, NamespaceMode::Required);
    assert_eq!(s.network, NetworkMode::Full);
    assert_eq!(s.allowed_domains, ["a.test"]);
    assert_eq!(s.extra_write_paths, ["/w"]);
    assert_eq!(s.extra_read_paths, ["/r"]);
    assert_eq!(s.deny_read_paths, ["/d"]);
    assert_eq!(s.cache_paths, ["/c"]);
    assert_eq!(s.allowed_ports, [8080]);

    assert!(!c.web.enabled && c.web.allow_private);
    assert_eq!(c.web.search.provider, "searxng");
    assert_eq!(c.web.search.url.as_deref(), Some("https://searx.example"));

    // Product sections.
    assert_eq!(c.mise.lazy_threshold, 33);
    assert!(!c.update.auto && !c.update.check);
}

#[test]
fn an_empty_config_is_the_defaults() {
    let c = load_str("").unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.default_profile, "anthropic");
    assert_eq!(c.compaction_threshold, 0.8);
    assert_eq!(c.max_turn_requests, 100);
    assert!(c.check_updates);
    assert_eq!(
        c.profiles.keys().collect::<Vec<_>>(),
        ["anthropic", "openai"]
    );
    assert_eq!(c.mcp.lazy_threshold, 20);
    assert_eq!(c.mise.lazy_threshold, 12);
    assert!(c.update.auto && c.update.check);
    assert!(c.mcp_servers.is_empty() && c.permissions.is_empty());
    assert!(c.sandbox.enabled && c.web.enabled);
    assert_eq!(c.sandbox.project_policy, ProjectPolicy::Remote);
}

#[test]
fn layers_apply_defaults_then_user_then_mcp_json_then_project() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "config.toml",
        r#"
default_profile = "u"
max_turn_requests = 10
compaction_threshold = 0.5
[profiles.u]
provider = "mock"
model = "user-model"
[profiles.anthropic]
model = "user-opus"
max_tokens = 111
[mcp_servers.s]
command = "user"
[permissions]
Bash = "deny"
Read = "deny"
"#,
    );
    write(
        proj.path(),
        ".mcp.json",
        r#"{"mcpServers": {"s": {"command": "json"}}}"#,
    );
    write(
        proj.path(),
        ".ruckcode/config.toml",
        r#"
max_turn_requests = 20
[profiles.anthropic]
model = "proj-opus"
[permissions]
Bash = "allow"
"#,
    );
    let c = Config::load(proj.path(), Some(user.path())).unwrap();
    // defaults < user: the default profile and the user-only profile.
    assert_eq!(c.default_profile, "u");
    assert_eq!(c.compaction_threshold, 0.5);
    assert_eq!(c.profiles["u"].model, "user-model");
    // user < project, merged per key.
    assert_eq!(c.max_turn_requests, 20);
    assert_eq!(c.profiles["anthropic"].model, "proj-opus");
    assert_eq!(c.profiles["anthropic"].max_tokens, 111);
    assert_eq!(c.profiles["anthropic"].context_window, 200_000);
    assert_eq!(c.permissions["Bash"], Permission::Allow);
    assert_eq!(c.permissions["Read"], Permission::Deny);
    // user < .mcp.json
    assert_eq!(c.mcp_servers["s"].command.as_deref(), Some("json"));
}

#[test]
fn a_project_sandbox_table_is_dropped() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(
        proj.path(),
        ".ruckcode/config.toml",
        "[sandbox]\nenabled = false\nproject_policy = \"off\"\nnetwork = \"full\"\nstrict = true\nextra_write_paths = [\"/x\"]\nbogus = 1\n",
    );
    // Even an invalid project [sandbox] (unknown key) is ignored, not an error.
    let c = Config::load(proj.path(), Some(user.path())).unwrap();
    assert_eq!(c.sandbox, Config::default().sandbox);
    assert!(c.sandbox.enabled);
    assert_eq!(c.sandbox.project_policy, ProjectPolicy::Remote);
}

#[test]
fn sandbox_enabled_and_project_policy_come_from_user_config_only() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "config.toml",
        "[sandbox]\nenabled = false\nproject_policy = \"off\"\n",
    );
    write(
        proj.path(),
        ".ruckcode/config.toml",
        "[sandbox]\nenabled = true\nproject_policy = \"remote\"\n",
    );
    let c = Config::load(proj.path(), Some(user.path())).unwrap();
    assert!(!c.sandbox.enabled);
    assert_eq!(c.sandbox.project_policy, ProjectPolicy::Off);
}

#[test]
fn a_project_web_table_keeps_only_enabled() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "config.toml",
        "[web]\nallow_private = true\n[web.search]\nprovider = \"searxng\"\nurl = \"https://u.test\"\n",
    );
    write(
        proj.path(),
        ".ruckcode/config.toml",
        "[web]\nenabled = false\nallow_private = false\nbogus = 1\n[web.search]\nurl = \"http://evil.test\"\n",
    );
    let c = Config::load(proj.path(), Some(user.path())).unwrap();
    assert!(!c.web.enabled);
    // The user's values survive; nothing else of the project's [web] is read (not even an
    // unknown key, which would otherwise be an error).
    assert!(c.web.allow_private);
    assert_eq!(c.web.search.provider, "searxng");
    assert_eq!(c.web.search.url.as_deref(), Some("https://u.test"));
}

#[test]
fn mcp_servers_merge_whole_entries_by_name_across_layers() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(
        user.path(),
        "config.toml",
        "[mcp_servers.a]\ncommand = \"ua\"\nargs = [\"1\"]\n[mcp_servers.b]\ncommand = \"ub\"\n",
    );
    write(
        proj.path(),
        ".mcp.json",
        r#"{"mcpServers": {"a": {"type": "http", "url": "http://j/mcp"}}}"#,
    );
    write(
        proj.path(),
        ".ruckcode/config.toml",
        "[mcp_servers.b]\nurl = \"http://p/mcp\"\n",
    );
    let c = Config::load(proj.path(), Some(user.path())).unwrap();
    assert_eq!(c.mcp_servers["a"].command, None);
    assert_eq!(c.mcp_servers["a"].url.as_deref(), Some("http://j/mcp"));
    assert_eq!(c.mcp_servers["b"].command, None);
    assert_eq!(c.mcp_servers["b"].url.as_deref(), Some("http://p/mcp"));
}

#[test]
fn malformed_files_name_the_file() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    write(proj.path(), ".ruckcode/config.toml", "this is not toml");
    let e = format!(
        "{:#}",
        Config::load(proj.path(), Some(user.path())).unwrap_err()
    );
    assert!(
        e.starts_with(&format!(
            "parsing {}",
            proj.path().join(".ruckcode/config.toml").display()
        )),
        "{e}"
    );
    let proj2 = tempfile::tempdir().unwrap();
    write(proj2.path(), ".mcp.json", "{nope");
    let e = format!("{:#}", Config::load(proj2.path(), None).unwrap_err());
    assert!(e.starts_with("parsing "), "{e}");
    assert!(e.contains(".mcp.json"), "{e}");
}

#[test]
fn validation_error_messages_are_pinned() {
    let base = "[profiles.p]\nprovider = \"openai\"\nmodel = \"m\"\n";
    assert_eq!(
        err("default_profile = \"nope\""),
        "default_profile `nope` is not defined"
    );
    assert_eq!(
        err("[profiles.anthropic.pricing]\ninput = -1.0\noutput = 1.0\n"),
        "profile `anthropic`: pricing rates must be non-negative numbers"
    );
    assert_eq!(
        err(&format!(
            "{base}[profiles.p.pricing]\ninput = 1.0\noutput = 1.0\ncache_write = -2.0\n"
        )),
        "profile `p`: pricing rates must be non-negative numbers"
    );
    assert_eq!(
        err(&format!(
            "{base}[profiles.p.model_pricing.x]\ninput = 1.0\noutput = -1.0\n"
        )),
        "profile `p`: pricing rates must be non-negative numbers"
    );
    assert_eq!(
        err(&format!("{base}temperature = 2.5\n")),
        "profile `p`: temperature must be between 0.0 and 2.0 (got 2.5)"
    );
    assert_eq!(
        err(&format!("{base}temperature = -0.1\n")),
        "profile `p`: temperature must be between 0.0 and 2.0 (got -0.1)"
    );
    assert_eq!(
        err(&format!("{base}top_p = 1.5\n")),
        "profile `p`: top_p must be between 0.0 and 1.0 (got 1.5)"
    );
    assert_eq!(
        err(&format!("{base}top_p = -1\n")),
        "profile `p`: top_p must be between 0.0 and 1.0 (got -1)"
    );
    // The boundaries are valid.
    assert!(load_str(&format!("{base}temperature = 2.0\ntop_p = 0.0\n")).is_ok());
    assert!(load_str(&format!("{base}temperature = 0.0\ntop_p = 1.0\n")).is_ok());
    // A profile missing its required keys.
    let e = err("[profiles.p]\nmodel = \"m\"\n");
    assert!(e.starts_with("invalid configuration"), "{e}");
    assert!(e.contains("missing field `provider`"), "{e}");
    // The default profile may come from a later layer, but must exist in the end.
    assert_eq!(
        err("default_profile = \"\""),
        "default_profile `` is not defined"
    );
}

#[test]
fn type_errors_are_wrapped_as_invalid_configuration() {
    for (text, needle) in [
        ("compaction_threshold = \"x\"", "invalid type"),
        ("check_updates = \"yes\"", "invalid type"),
        ("[mise]\nlazy_threshold = \"x\"", "invalid type"),
        ("[update]\nauto = 3", "invalid type"),
        ("[permissions]\nBash = \"maybe\"", "maybe"),
    ] {
        let e = err(text);
        assert!(e.starts_with("invalid configuration: "), "{text}: {e}");
        assert!(e.contains(needle), "{text}: {e}");
    }
}

#[test]
fn unknown_keys_follow_each_types_own_rule() {
    // Config, Profile, Pricing, [mise], [update], [mcp], [context], [sandbox], the guards and
    // the rate limit ignore unknown keys...
    let c = load_str(
        "bogus = 1\n[bogus_table]\nx = 1\n[mise]\nbogus = 1\n[update]\nbogus = 1\n[mcp]\nbogus = 1\n\
         [rate_limit]\nbogus = 1\n[context]\nbogus = 1\n[sandbox]\nbogus = 1\n[thinking_guard]\nbogus = 1\n\
         [tool_loop_guard]\nbogus = 1\n[profiles.anthropic]\nbogus = 1\n\
         [profiles.anthropic.pricing]\ninput = 1.0\noutput = 1.0\nbogus = 1\n",
    )
    .unwrap();
    assert_eq!(c.default_profile, "anthropic");
    // ...while [web] (and [web.search]) reject them.
    for text in ["[web]\nbogus = 1\n", "[web.search]\nbogus = 1\n"] {
        let e = err(text);
        assert!(e.starts_with("invalid configuration: "), "{text}: {e}");
        assert!(e.contains("unknown field `bogus`"), "{text}: {e}");
    }
}
