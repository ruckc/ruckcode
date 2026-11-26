//! Pins the `modes_api` / `modes_edit` behaviour around `mise_tasks` and unknown keys, ahead of
//! moving `mise_tasks` into `ModeDef.extra`. Output is compared byte for byte.

use ruckcode_core::modes_api::{apply, catalog, parse_doc, preview};
use ruckcode_core::ModeSet;

const SRC: &str = "\
# my modes
default_mode = \"build\"

# the build mode
[modes.build]
description = \"Build it\"   # trailing
mise_tasks = [\"build\", \"test*\"]   # tasks
bogus = 1
mcp_tools = [\"mcp__*\"]

[modes.docs]
description = \"Docs\"
";

fn project(text: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".ruckcode")).unwrap();
    std::fs::write(d.path().join(".ruckcode/modes.toml"), text).unwrap();
    d
}

fn read(d: &tempfile::TempDir) -> String {
    std::fs::read_to_string(d.path().join(".ruckcode/modes.toml")).unwrap()
}

#[test]
fn unchanged_doc_rewrites_nothing() {
    let d = project(SRC);
    let doc = parse_doc(SRC).unwrap();
    assert_eq!(doc.modes[0].mise_tasks, ["build", "test*"]);
    assert!(doc.modes[1].mise_tasks.is_empty());
    let p = preview(d.path(), None, &doc).unwrap();
    assert!(!p.changed);
    assert!(!apply(d.path(), None, &doc).unwrap().changed);
    assert_eq!(read(&d), SRC);
}

#[test]
fn editing_mise_tasks_keeps_comments_and_unknown_keys_in_the_file() {
    let d = project(SRC);
    let mut doc = parse_doc(SRC).unwrap();
    doc.modes[0].mise_tasks = vec!["lint".into(), "fmt".into()];
    doc.modes[1].mise_tasks = vec!["docs*".into()];
    apply(d.path(), None, &doc).unwrap();
    assert_eq!(
        read(&d),
        "\
# my modes
default_mode = \"build\"

# the build mode
[modes.build]
description = \"Build it\"   # trailing
mise_tasks = [\"lint\", \"fmt\"]   # tasks
bogus = 1
mcp_tools = [\"mcp__*\"]

[modes.docs]
description = \"Docs\"
mise_tasks = [\"docs*\"]
"
    );
}

#[test]
fn clearing_mise_tasks_removes_the_key() {
    let d = project(SRC);
    let mut doc = parse_doc(SRC).unwrap();
    doc.modes[0].mise_tasks.clear();
    apply(d.path(), None, &doc).unwrap();
    assert_eq!(
        read(&d),
        "\
# my modes
default_mode = \"build\"

# the build mode
[modes.build]
description = \"Build it\"   # trailing
bogus = 1
mcp_tools = [\"mcp__*\"]

[modes.docs]
description = \"Docs\"
"
    );
}

#[test]
fn mode_dto_json_has_mise_tasks_and_never_unknown_keys() {
    let doc = parse_doc(SRC).unwrap();
    let v = serde_json::to_value(&doc.modes[0]).unwrap();
    assert_eq!(v["miseTasks"], serde_json::json!(["build", "test*"]));
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert!(!keys.contains(&"bogus") && !keys.contains(&"extra"), "{keys:?}");
    assert_eq!(
        serde_json::to_string(&doc.modes[1]).unwrap(),
        "{\"id\":\"docs\",\"name\":null,\"description\":\"Docs\",\"prompt\":\"\",\"tools\":null,\"miseTasks\":[],\"mcpTools\":[],\"next\":[],\"autoAdvanceTo\":[],\"profile\":null,\"model\":null,\"permissions\":{},\"hooks\":{}}"
    );
}

#[tokio::test]
async fn catalog_templates_carry_mise_tasks() {
    let d = tempfile::tempdir().unwrap();
    let c = catalog(d.path(), None).await;
    let json = serde_json::to_value(&c.templates).unwrap();
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["mode"]["miseTasks"].is_array()));
    let want = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/modes.txt"
    ))
    .unwrap();
    for t in c.templates.iter().filter(|t| t.set == "full") {
        let line = want
            .lines()
            .find(|l| l.starts_with(&format!("MODE {} ", t.mode.id)))
            .unwrap();
        assert!(
            line.contains(&format!("mise={:?}", t.mode.mise_tasks)),
            "{}: {line}",
            t.mode.id
        );
    }
}

#[test]
fn modeset_rejects_ill_typed_mise_tasks() {
    let s = ModeSet::from_toml_str(SRC).unwrap();
    assert_eq!(s.get("build").unwrap().mise_tasks, ["build", "test*"]);
    let e = ModeSet::from_toml_str("[modes.a]\nmise_tasks = \"x\"\n").unwrap_err();
    assert_eq!(e.to_string(), "invalid mode `a`");
    let e = ModeSet::from_toml_str("[modes.a]\nmise_tasks = [1]\n").unwrap_err();
    assert_eq!(e.to_string(), "invalid mode `a`");
}
