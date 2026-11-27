//! The tool-loop guard: repeated no-progress tool calls are dropped from the history, nudged,
//! and end the turn after too many nudges; legitimate progress never trips it.

use async_trait::async_trait;
use ruck_harness_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruck_harness_core::transcript::{session_path, Record, TranscriptReader};
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_llm::{ContentBlock, Message, Request, Role};
use ruck_harness_tools::{Answer, Question, UserBridge};
use ruckcode_agent::extension::RuckcodeAgent as _;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const NUDGE: &str =
    "You are repeating the same tool call without progress. Take a different next step, or answer.";

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl TestClient {
    fn notices(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }
    fn plans(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, SessionEvent::Plan(_)))
            .count()
    }
    fn outputs(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::ToolCallUpdate {
                    output: Some(o), ..
                } => Some(o.clone()),
                _ => None,
            })
            .collect()
    }
}

fn call(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

struct Run {
    stop: StopReason,
    client: Arc<TestClient>,
    mock: Arc<MockProvider>,
    records: Vec<Record>,
}

/// Run one prompt against the scripted `steps` with `config` as the project config.
async fn run(steps: Vec<Message>, config: &str) -> Run {
    run_with(steps, config, false).await
}

/// Like [`run`]; with `git` the project is a git repository whose `src/x.ts` and the `lib/`
/// search fixtures are committed.
async fn run_with(steps: Vec<Message>, config: &str, git: bool) -> Run {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
    std::fs::write(proj.path().join(".ruckcode/config.toml"), config).unwrap();
    std::fs::write(
        user.path().join("config.toml"),
        "[sandbox]\nenabled = false\n",
    )
    .unwrap();
    for n in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        std::fs::write(proj.path().join(format!("{n}.txt")), format!("file {n}")).unwrap();
    }
    std::fs::create_dir_all(proj.path().join("src")).unwrap();
    std::fs::write(proj.path().join("src/x.ts"), "import ENV\n").unwrap();
    std::fs::create_dir_all(proj.path().join("lib/app")).unwrap();
    std::fs::write(
        proj.path().join("lib/app/x.js"),
        "export const base = 1;\nexport const base_internal = 2;\n",
    )
    .unwrap();
    std::fs::write(proj.path().join("lib/other.js"), "export const base = 3;\n").unwrap();
    for i in 0..8 {
        std::fs::write(
            proj.path().join(format!("lib/f{i}.js")),
            format!("export const base = {i};\n"),
        )
        .unwrap();
    }
    if git {
        for args in [
            vec!["init", "-q"],
            vec!["add", "src", "lib"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        ] {
            let st = std::process::Command::new("git")
                .args(&args)
                .current_dir(proj.path())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        }
    }
    let mock = Arc::new(MockProvider::new(steps));
    let agent = Agent::ruckcode(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let stop = agent
        .prompt(&info.id, "design a workflow".into(), client.clone())
        .await
        .unwrap();
    let records = TranscriptReader::load(&session_path(
        &ruckcode_agent::identity::RUCKCODE,
        proj.path(),
        &info.id,
    ))
    .unwrap()
    .records
    .into_iter()
    .map(|r| r.record)
    .collect();
    Run {
        stop,
        client,
        mock,
        records,
    }
}

fn plan(statuses: [&str; 3]) -> Value {
    let items: Vec<Value> = ["draft", "review", "ship"]
        .iter()
        .zip(statuses)
        .map(|(c, s)| json!({"content": c, "status": s}))
        .collect();
    json!({ "todos": items })
}

fn tool_uses(r: &Request) -> usize {
    r.messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .count()
}

#[tokio::test]
async fn repeated_noop_plan_updates_are_local_dropped_nudged_and_end_the_turn() {
    let same = plan(["pending", "pending", "pending"]);
    let steps = (0..10)
        .map(|i| call(&format!("t{i}"), "TodoWrite", same.clone()))
        .collect();
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert_eq!(r.stop.as_str(), "tool_loop");
    // Only the first update ran and reached clients; the other nine were answered locally.
    assert_eq!(r.client.plans(), 1);
    let outputs = r.client.outputs();
    assert_eq!(outputs.len(), 10);
    assert!(outputs[0].starts_with("Plan updated"));
    assert!(outputs[1..]
        .iter()
        .all(|o| o == "Plan unchanged (no items or statuses changed)."));
    // 1 real + 3 no-ops trips; nudged twice; the third trip ends the turn.
    let notices = r.client.notices();
    assert_eq!(notices.len(), 3, "{notices:?}");
    assert!(notices[0].contains("nudge 1/2") && notices[1].contains("nudge 2/2"));
    assert!(notices[2].contains("stopping this turn after 2 nudge"));
    let reqs = r.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 10);
    // The request after the first trip carries the first update only, and the nudge merged
    // into the tool-result message so roles still alternate.
    let after = &reqs[4];
    assert_eq!(tool_uses(after), 1);
    assert_eq!(after.messages.len(), 3);
    assert_eq!(after.messages[2].role, Role::User);
    assert!(after.messages[2].text().contains(NUDGE));
    // Every request is bounded: the history never regrows past one kept call + one nudge each.
    assert!(reqs.iter().all(|q| tool_uses(q) <= 3));
    // The turn is unfinished and the drops are recorded for resume.
    assert!(r
        .records
        .iter()
        .any(|x| matches!(x, Record::ToolCallsDropped { .. })));
    assert!(r
        .records
        .iter()
        .any(|x| matches!(x, Record::TurnEnd { stop_reason, .. } if stop_reason == "tool_loop")));
}

#[tokio::test]
async fn identical_reads_trip_the_guard_and_drop_the_repeats() {
    let mut steps: Vec<Message> = (0..4)
        .map(|i| call(&format!("r{i}"), "Read", json!({"file_path": "a.txt"})))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    let notices = r.client.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("`Read` 4 times"), "{notices:?}");
    let reqs = r.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    assert_eq!(tool_uses(&reqs[4]), 1);
    assert!(reqs[4].messages.last().unwrap().text().contains(NUDGE));
}

#[tokio::test]
async fn zero_nudges_stops_on_the_first_trip() {
    let steps: Vec<Message> = (0..4)
        .map(|i| call(&format!("r{i}"), "Read", json!({"file_path": "a.txt"})))
        .collect();
    let r = run(steps, "[tool_loop_guard]\nmax_nudges = 0\n").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert_eq!(r.mock.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn reading_different_files_is_progress() {
    let mut steps: Vec<Message> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .iter()
        .map(|n| call(n, "Read", json!({"file_path": format!("{n}.txt")})))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
}

#[tokio::test]
async fn same_command_with_changing_output_is_progress() {
    let cmd = "n=$(cat counter 2>/dev/null || echo 0); echo $((n+1)) > counter; echo run $n";
    let mut steps: Vec<Message> = (0..8)
        .map(|i| call(&format!("b{i}"), "Bash", json!({ "command": cmd })))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty(), "{:?}", r.client.notices());
    let outs = r.client.outputs();
    assert!(outs.iter().any(|o| o.contains("run 7")), "{outs:?}");
}

#[tokio::test]
async fn plan_with_real_changes_is_progress_and_completion_still_reported() {
    let steps = vec![
        call("p0", "TodoWrite", plan(["pending", "pending", "pending"])),
        call(
            "p1",
            "TodoWrite",
            plan(["in_progress", "pending", "pending"]),
        ),
        call(
            "p2",
            "TodoWrite",
            plan(["completed", "in_progress", "pending"]),
        ),
        call(
            "p3",
            "TodoWrite",
            plan(["completed", "completed", "in_progress"]),
        ),
        call(
            "p4",
            "TodoWrite",
            plan(["completed", "completed", "completed"]),
        ),
        Message::assistant_text("all done"),
    ];
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
    assert_eq!(r.client.plans(), 5);
    assert!(r
        .client
        .outputs()
        .last()
        .unwrap()
        .contains("All plan items are complete"));
}

#[tokio::test]
async fn disabled_guard_runs_everything() {
    let same = plan(["pending", "pending", "pending"]);
    let mut steps: Vec<Message> = (0..10)
        .map(|i| call(&format!("t{i}"), "TodoWrite", same.clone()))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "[tool_loop_guard]\nenabled = false\n").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.client.plans(), 10);
    assert!(r.client.notices().is_empty());
}

fn bash(id: &str, command: &str) -> Message {
    call(id, "Bash", json!({ "command": command }))
}

fn write(id: &str, content: &str) -> Message {
    call(
        id,
        "Write",
        json!({"file_path": "src/x.ts", "content": content}),
    )
}

const GREP_VARIANTS: [&str; 6] = [
    r#"grep -r "export const base" lib"#,
    r#"grep -r "export const base" lib/app"#,
    r#"grep -r "export const base" lib/app | grep -v internal"#,
    r#"grep -r 'export  const base' lib | grep -v zzzzzz"#,
    r#"grep -r "export const base" lib/app lib/other.js"#,
    r#"grep -r "export const base" lib/app 2>/dev/null | head -50"#,
];

#[tokio::test]
async fn repeated_search_with_changing_arguments_but_no_new_results_trips() {
    let mut steps: Vec<Message> = GREP_VARIANTS
        .iter()
        .enumerate()
        .map(|(i, c)| bash(&format!("g{i}"), c))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    let notices = r.client.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("searched for `export const base` 6 times with no new results"),
        "{notices:?}"
    );
    let reqs = r.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 7);
    let last = reqs[6].messages.last().unwrap().text();
    assert!(last.contains(
        "You have searched for `export const base` 6 times with no new results. Use what you \
         found, or take a different approach."
    ));
    // The first search that found everything stays; redundant later ones are dropped.
    assert!(tool_uses(&reqs[6]) < 6 && tool_uses(&reqs[6]) >= 1);
    assert!(r
        .records
        .iter()
        .any(|x| matches!(x, Record::ToolCallsDropped { .. })));
}

#[tokio::test]
async fn grep_tool_searches_count_too_and_zero_nudges_end_the_turn() {
    let steps: Vec<Message> = (0..6)
        .map(|i| {
            call(
                &format!("g{i}"),
                "Grep",
                json!({"pattern": "export const base", "path": (["lib", "lib/app", "lib/app/x.js", "lib/other.js", "lib/app", "lib/app/x.js"])[i], "output_mode": "content"}),
            )
        })
        .collect();
    let r = run(steps, "[tool_loop_guard]\nmax_nudges = 0\n").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert!(r
        .client
        .notices()
        .iter()
        .any(|n| n.contains("searched for `export const base` 6 times")));
}

#[tokio::test]
async fn refined_searches_that_find_new_matches_do_not_trip() {
    let mut steps: Vec<Message> = (0..8)
        .map(|i| {
            bash(
                &format!("g{i}"),
                &format!(r#"grep "export const base" lib/f{i}.js"#),
            )
        })
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty(), "{:?}", r.client.notices());
}

#[tokio::test]
async fn search_detector_can_be_disabled_or_tuned() {
    for cfg in [
        "[tool_loop_guard]\nsearch_repeat_enabled = false\n",
        "[tool_loop_guard]\nsearch_repeat_count = 0\n",
        "[tool_loop_guard]\nsearch_repeat_count = 10\n",
        "[tool_loop_guard]\nenabled = false\n",
    ] {
        let mut steps: Vec<Message> = GREP_VARIANTS
            .iter()
            .enumerate()
            .map(|(i, c)| bash(&format!("g{i}"), c))
            .collect();
        steps.push(Message::assistant_text("done"));
        let r = run(steps, cfg).await;
        assert_eq!(r.stop, StopReason::EndTurn, "{cfg}");
        assert!(
            r.client.notices().is_empty(),
            "{cfg}: {:?}",
            r.client.notices()
        );
    }
}

#[tokio::test]
async fn write_oscillation_trips_names_the_file_and_keeps_edits() {
    let states = ["import PATHS\n", "import ENVIRONMENT\n", "import ENV\n"];
    let mut steps: Vec<Message> = (0..4)
        .map(|i| write(&format!("w{i}"), states[i % 3]))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    let notices = r.client.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("back and forth") && notices[0].contains("x.ts"),
        "{notices:?}"
    );
    assert!(!notices[0].contains("dropped"), "{notices:?}");
    let reqs = r.mock.requests.lock().unwrap();
    let last = reqs[4].messages.last().unwrap().text();
    assert!(
        last.contains("back and forth between the same versions. Decide on one change, or answer."),
        "{last}"
    );
    assert!(last.contains("x.ts"));
    // Edits stay in the history.
    assert_eq!(tool_uses(&reqs[4]), 4);
    assert!(!r
        .records
        .iter()
        .any(|x| matches!(x, Record::ToolCallsDropped { .. })));
}

#[tokio::test]
async fn oscillation_that_never_stops_ends_the_turn() {
    let states = ["import PATHS\n", "import ENVIRONMENT\n", "import ENV\n"];
    let steps: Vec<Message> = (0..30)
        .map(|i| write(&format!("w{i}"), states[i % 3]))
        .collect();
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert!(r
        .client
        .notices()
        .iter()
        .any(|n| n.contains("stopping this turn after 2 nudge")));
}

#[tokio::test]
async fn sed_oscillation_in_a_git_repo_trips() {
    let seds = [
        "sed -i 's/ENV/PATHS/' src/x.ts",
        "sed -i 's/PATHS/ENVIRONMENT/' src/x.ts",
        "sed -i 's/ENVIRONMENT/ENV/' src/x.ts",
    ];
    let mut steps: Vec<Message> = (0..6)
        .map(|i| bash(&format!("s{i}"), seds[i % 3]))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run_with(steps.clone(), "", true).await;
    let notices = r.client.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("x.ts") && notices[0].contains("back and forth"),
        "{notices:?}"
    );
    // Without a git repository, Bash edits cannot be seen.
    let r = run_with(steps.clone(), "", false).await;
    assert!(r.client.notices().is_empty(), "{:?}", r.client.notices());
    // Disabled.
    let r = run_with(
        steps,
        "[tool_loop_guard]\noscillation_enabled = false\n",
        true,
    )
    .await;
    assert!(r.client.notices().is_empty());
}

#[tokio::test]
async fn a_single_deliberate_revert_does_not_trip() {
    let steps = vec![
        write("w0", "import PATHS\n"),
        write("w1", "import ENV\n"),
        write("w2", "import ENV2\n"),
        Message::assistant_text("done"),
    ];
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty(), "{:?}", r.client.notices());
}

#[tokio::test]
async fn progressive_edits_to_one_file_do_not_trip() {
    let mut steps: Vec<Message> = (0..12)
        .map(|i| write(&format!("w{i}"), &format!("import V{i}\n")))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
}

#[tokio::test]
async fn disabled_guard_ignores_oscillation_and_searches() {
    let states = ["import PATHS\n", "import ENVIRONMENT\n", "import ENV\n"];
    let mut steps: Vec<Message> = (0..9)
        .map(|i| write(&format!("w{i}"), states[i % 3]))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "[tool_loop_guard]\nenabled = false\n").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
}
