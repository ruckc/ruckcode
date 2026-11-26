//! System prompt assembly.

use std::path::Path;

use crate::config::Profile;
use crate::modes::ModeSet;

const BASE: &str = "\
You are ruckcode, an interactive coding agent running in the user's terminal or editor. You help \
with software engineering tasks by reading, searching, editing and running code with the tools \
provided.

- Be concise and direct. Do the work rather than describing it; do not narrate each step.
- Prefer the dedicated tools (Read, Glob, Grep, Edit, Write) over shell commands for files.
- Read code before changing it and match the style of the surrounding code.
- Tools named `mise__*` (or MiseRun) run the project's own mise tasks; prefer them for builds, \
tests, linting and other project workflows.
{TODO}{ASK}{WEB}- Work happens in lifecycle modes. When the current phase's work is done, use SwitchMode to \
propose moving to the next phase; the user may need to approve.
- Do not commit, push or publish unless explicitly asked.";

const TODO_LINE: &str = "- Use TodoWrite to plan and track multi-step work. When the last item is completed, \
tell the user the plan is finished and summarise what was done, then stop unless they ask for more.\n";
const ASK_LINE: &str =
    "- Use AskUserQuestion when you are blocked on a decision only the user can make.\n";

const WEB_LINE: &str =
    "- Use WebSearch to find pages, then WebFetch on specific URLs to read them.\n";
const WEB_FETCH_LINE: &str = "- Use WebFetch to read a specific URL.\n";
const WEB_SEARCH_LINE: &str = "- Use WebSearch to look things up on the web.\n";

/// The built-in guidance, leaving out the lines about built-in tools the profile does not offer
/// (a model must not be told to use a tool it cannot call).
fn base_prompt(profile: Option<&Profile>) -> String {
    let offers = |name: &str| profile.is_none_or(|p| p.allows_builtin(name));
    let web = match (offers("WebSearch"), offers("WebFetch")) {
        (true, true) => WEB_LINE,
        (false, true) => WEB_FETCH_LINE,
        (true, false) => WEB_SEARCH_LINE,
        (false, false) => "",
    };
    BASE.replace("{WEB}", web)
        .replace("{TODO}", if offers("TodoWrite") { TODO_LINE } else { "" })
        .replace(
            "{ASK}",
            if offers("AskUserQuestion") {
                ASK_LINE
            } else {
                ""
            },
        )
}

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".ruckcode/INSTRUCTIONS.md"];

async fn git_branch(cwd: &Path) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Longest accepted client instruction text, in bytes.
pub const MAX_CLIENT_INSTRUCTIONS: usize = 64 * 1024;

/// Build the system prompt: built-in guidance, then the project's instruction files and the
/// current mode, then the client's appended instructions (which never replace anything above).
pub async fn system_prompt(
    cwd: &Path,
    modes: &ModeSet,
    mode_id: &str,
    client_instructions: Option<&str>,
    profile: Option<&Profile>,
) -> String {
    let mut s = base_prompt(profile);
    s.push_str("\n\n## Environment\n");
    s.push_str(&format!("- Working directory: {}\n", cwd.display()));
    s.push_str(&format!("- Platform: {}\n", std::env::consts::OS));
    s.push_str(&format!(
        "- Today's date: {}\n",
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    if let Some(b) = git_branch(cwd).await {
        s.push_str(&format!("- Git branch: {b}\n"));
    }
    for f in INSTRUCTION_FILES {
        if let Ok(text) = std::fs::read_to_string(cwd.join(f)) {
            s.push_str(&format!(
                "\n## Project instructions ({f})\n{}\n",
                text.trim()
            ));
        }
    }
    if let Some(mode) = modes.get(mode_id) {
        s.push_str(&format!(
            "\n## Current mode: {}\n{}\n{}\n",
            modes.display_name(mode_id),
            mode.description,
            mode.prompt
        ));
    }
    if modes.modes.len() > 1 {
        s.push_str("\n## Modes\n");
        for (id, m) in &modes.modes {
            s.push_str(&format!("- {id}: {}\n", m.description));
        }
    }
    if let Some(c) = client_instructions.map(str::trim).filter(|c| !c.is_empty()) {
        s.push_str(&format!(
            "\n## Client instructions\nThe client that started this session supplied the following \
instructions. They add to, and never override, the guidance above.\n\n{c}\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_keeps_the_todo_and_ask_lines() {
        let s = base_prompt(None);
        assert!(s.contains("Use TodoWrite") && s.contains("Use AskUserQuestion"));
        assert!(!s.contains('{'));
        assert!(s.contains("workflows.\n- Use TodoWrite"));
    }

    #[test]
    fn web_lines_follow_the_profile() {
        let prof = |extra: &str| -> Profile {
            toml::from_str(&format!("provider = \"openai\"\nmodel = \"m\"\n{extra}")).unwrap()
        };
        assert!(base_prompt(None).contains("Use WebSearch to find pages, then WebFetch"));
        assert!(base_prompt(Some(&prof(""))).contains("Use WebSearch to find pages"));
        let lean = base_prompt(Some(&prof("lean_tools = true\n")));
        assert!(!lean.contains("Web") && !lean.contains('{'), "{lean}");
        let only_fetch = base_prompt(Some(&prof("tools = [\"Read\", \"WebFetch\"]\n")));
        assert!(only_fetch.contains("Use WebFetch to read") && !only_fetch.contains("WebSearch"));
        let lean_search = base_prompt(Some(&prof("lean_tools = true\ntools = [\"WebSearch\"]\n")));
        assert!(lean_search.contains("Use WebSearch to look") && !lean_search.contains("WebFetch"));
    }

    #[test]
    fn lean_profile_drops_the_lines_about_dropped_tools() {
        let p: Profile =
            toml::from_str("provider = \"openai\"\nmodel = \"m\"\nlean_tools = true\n").unwrap();
        let s = base_prompt(Some(&p));
        assert!(!s.contains("TodoWrite") && !s.contains("AskUserQuestion"));
        assert!(s.contains("workflows.\n- Work happens in lifecycle modes"));
    }
}
