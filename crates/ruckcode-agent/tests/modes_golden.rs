//! Golden parse of the shipped `templates/modes*.toml`: the parsed `ModeSet` must not change.
//! `prompthash` is a polynomial hash of the prompt text, so any change to a prompt shows up.

use ruck_harness_core::ModeSet;
use ruckcode_agent::mode_ext::RuckcodeModeExt;
use ruckcode_agent::mode_ext::RuckcodeModeSet as _;

fn summary(s: &ModeSet) -> String {
    let mut out = format!("DEFAULT {}\n", s.default_mode);
    for (id, m) in &s.modes {
        out.push_str(&format!(
            "MODE {id} name={:?} desc={:?} tools={:?} mise={:?} mcp={:?} next={:?} auto={:?} prof={:?} model={:?} perms={:?} hooks={} sandbox={:?} promptlen={} prompthash={}\n",
            m.name, m.description, m.tools, m.mise_tasks(), m.mcp_tools, m.next, m.auto_advance_to,
            m.profile, m.model, m.permissions, m.hooks.post_edit.len(), m.sandbox, m.prompt.len(),
            m.prompt.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64))
        ));
    }
    out
}

#[test]
fn shipped_full_template_parses_to_the_golden_modeset() {
    let s = ModeSet::from_toml_str(include_str!("../templates/modes.toml")).unwrap();
    assert_eq!(summary(&s), include_str!("golden/modes.txt"));
    assert_eq!(
        s.ids(),
        [
            "explore",
            "design",
            "implement",
            "test",
            "review",
            "release"
        ]
    );
}

#[test]
fn shipped_minimal_template_parses_to_the_golden_modeset() {
    let s = ModeSet::from_toml_str(include_str!("../templates/modes-minimal.toml")).unwrap();
    assert_eq!(summary(&s), include_str!("golden/modes-minimal.txt"));
    assert_eq!(s.ids(), ["plan", "build", "verify"]);
}

#[test]
fn the_implicit_set_is_pinned() {
    let s = ModeSet::implicit();
    assert_eq!(s.default_mode, "default");
    assert_eq!(s.ids(), ["default"]);
    let d = s.get("default").unwrap();
    assert_eq!(d.mise_tasks(), ["*"]);
    assert_eq!(d.mcp_tools, ["mcp__*"]);
    assert_eq!(d.description, "General-purpose mode with all tools");
    assert!(d.tools.is_none() && d.next.is_empty());
}

#[test]
fn modedef_ignores_unknown_keys_but_modeset_validates_references() {
    // ModeDef has no deny_unknown_fields.
    let s = ModeSet::from_toml_str("[modes.a]\nbogus = 1\ndescription = \"d\"\n").unwrap();
    assert_eq!(s.get("a").unwrap().description, "d");
    let e = ModeSet::from_toml_str("[modes.a]\nnext = [\"zz\"]\n").unwrap_err();
    assert_eq!(e.to_string(), "mode `a` references unknown mode `zz`");
    let e = ModeSet::from_toml_str("default_mode = \"q\"\n[modes.a]\n").unwrap_err();
    assert_eq!(e.to_string(), "default_mode `q` is not defined");
    let e = ModeSet::from_toml_str("").unwrap_err();
    assert_eq!(e.to_string(), "modes.toml defines no modes");
}
