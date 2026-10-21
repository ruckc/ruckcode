use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use ruckcode_core::Config;

const MODES: &str = "default_mode = \"explore\"\n[modes.explore]\ndescription = \"look\"\n[modes.test]\ndescription = \"test\"\n";

struct Env {
    user: tempfile::TempDir,
    proj: tempfile::TempDir,
}

impl Env {
    fn new(with_modes: bool) -> Self {
        let e = Self {
            user: tempfile::tempdir().unwrap(),
            proj: tempfile::tempdir().unwrap(),
        };
        if with_modes {
            std::fs::create_dir_all(e.proj.path().join(".ruckcode")).unwrap();
            std::fs::write(e.proj.path().join(".ruckcode/modes.toml"), MODES).unwrap();
        }
        e
    }

    fn wizard(&self) -> SetupWizard {
        let mut ctx = SetupContext::load(self.proj.path(), Some(self.user.path()));
        ctx.env = Box::new(|k| (k == "OPENAI_API_KEY").then(|| "sk-env".to_string()));
        SetupWizard::new(ctx)
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(w: &mut SetupWizard, codes: &[KeyCode]) -> Effect {
    let mut last = Effect::None;
    for c in codes {
        last = w.handle_key(key(*c));
    }
    last
}

fn typ(w: &mut SetupWizard, s: &str) {
    for c in s.chars() {
        w.handle_key(key(KeyCode::Char(c)));
    }
}

fn models(ids: &[&str]) -> Vec<ModelInfo> {
    ids.iter()
        .map(|i| ModelInfo {
            id: i.to_string(),
            display_name: None,
        })
        .collect()
}

fn draw(w: &SetupWizard) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| render::render(w, f)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

/// Advance an OpenRouter wizard with an env-var key to the Models step.
fn to_models(w: &mut SetupWizard) -> FetchRequest {
    // Provider: OpenRouter is the fourth preset.
    press(
        w,
        &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter],
    );
    assert_eq!(w.step, Step::Endpoint);
    assert_eq!(w.endpoint.value, "https://openrouter.ai/api/v1");
    press(w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Auth);
    assert_eq!(w.env_name.value, "OPENROUTER_API_KEY");
    assert_eq!(w.auth_choice(), Some(AuthChoice::Env));
    press(w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    assert_eq!(w.profile_name.value, "openrouter");
    match press(w, &[KeyCode::Enter]) {
        Effect::FetchModels(r) => r,
        e => panic!("expected fetch, got {e:?}"),
    }
}

#[test]
fn full_walk_with_new_key_modes_and_apply() {
    let env = Env::new(true);
    let mut w = env.wizard();
    assert_eq!(w.step, Step::Provider);
    press(
        &mut w,
        &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter],
    );
    press(&mut w, &[KeyCode::Enter]);
    // Auth: new named key (row 2): keep the prefilled name, tab to the secret.
    press(&mut w, &[KeyCode::Down, KeyCode::Down]);
    assert_eq!(w.auth_choice(), Some(AuthChoice::New));
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Auth, "secret is required");
    assert!(w.error.is_some());
    typ(&mut w, "sk-or-secret-1234");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    let Effect::FetchModels(req) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert_eq!(req.api_key.as_deref(), Some("sk-or-secret-1234"));
    assert_eq!(req.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(req.kind, ApiKind::Openai);
    assert!(draw(&w).contains("fetching models"));
    w.models_loaded(req.generation, Ok(models(&["a/one", "b/two", "c/three"])));
    // Enter with nothing selected is refused.
    press(&mut w, &[KeyCode::Enter]);
    assert!(w.error.is_some());
    // Select first and third.
    press(
        &mut w,
        &[
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Char(' '),
        ],
    );
    press(&mut w, &[KeyCode::Enter]);
    assert!(
        matches!(w.models, ModelsPhase::Default { .. }),
        "{:?}",
        w.models
    );
    press(&mut w, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.default_model, "c/three");
    assert_eq!(w.step, Step::Modes);
    // Assign explore: chooser item 0 = default, 1 = openrouter/a/one.
    press(&mut w, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(
        w.assignments[0],
        Some(("openrouter".to_string(), "a/one".to_string()))
    );
    assert_eq!(w.assignments[1], None);
    press(&mut w, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.step, Step::Review);
    assert!(!w.preview.contains("sk-or"), "{}", w.preview);
    assert!(w.preview.contains("openrouter"), "{}", w.preview);
    let plan = w.plan();
    assert_eq!(plan.profile.models, ["a/one", "c/three"]);
    assert_eq!(plan.profile.api_key_name.as_deref(), Some("openrouter"));
    assert_eq!(plan.mode_models.len(), 2);
    assert!(plan.set_default);
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::Apply);
    let r = setup::apply(&w.plan(), Some(env.user.path()), env.proj.path()).unwrap();
    w.applied(Ok(r));
    assert!(w.done() && !w.aborted);
    let cfg = Config::load(env.proj.path(), Some(env.user.path())).unwrap();
    assert_eq!(cfg.default_profile, "openrouter");
    assert_eq!(cfg.profiles["openrouter"].model, "c/three");
    assert_eq!(
        ruckcode_core::config::named_key(Some(env.user.path()), "openrouter").as_deref(),
        Some("sk-or-secret-1234")
    );
    let modes = std::fs::read_to_string(env.proj.path().join(".ruckcode/modes.toml")).unwrap();
    assert!(modes.contains("model = \"a/one\""), "{modes}");
    let cfg_text = std::fs::read_to_string(env.user.path().join("config.toml")).unwrap();
    assert!(!cfg_text.contains("sk-or-secret"));
}

#[test]
fn esc_goes_back_and_first_step_aborts() {
    let env = Env::new(true);
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(req.generation, Ok(models(&["m1"])));
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::ProfileName);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Auth);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Endpoint);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Provider);
    assert!(!w.aborted);
    press(&mut w, &[KeyCode::Esc]);
    assert!(w.aborted && w.done());
}

#[test]
fn going_back_to_models_reuses_the_fetched_list() {
    let env = Env::new(true);
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(req.generation, Ok(models(&["m1", "m2"])));
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]); // m1 only -> straight to modes
    assert_eq!(w.step, Step::Modes);
    press(&mut w, &[KeyCode::Esc]);
    assert_eq!(w.step, Step::Models);
    press(&mut w, &[KeyCode::Esc]);
    press(&mut w, &[KeyCode::Enter]); // profile name -> models again
    assert_eq!(w.step, Step::Models);
    assert!(
        matches!(w.models, ModelsPhase::List(_)),
        "no refetch expected"
    );
    assert!(w.selected.contains("m1"));
}

#[test]
fn multi_select_all_and_filter() {
    let env = Env::new(false);
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(
        req.generation,
        Ok(models(&[
            "anthropic/claude-a",
            "openai/gpt",
            "openai/gpt-mini",
        ])),
    );
    // `a` with an empty filter selects all.
    press(&mut w, &[KeyCode::Char('a')]);
    assert_eq!(w.selected.len(), 3);
    press(&mut w, &[KeyCode::Char('a')]);
    assert!(w.selected.is_empty());
    // Typing filters (and `a` is part of the filter once it is non-empty).
    typ(&mut w, "gpt");
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert_eq!(st.visible().len(), 2);
    press(&mut w, &[KeyCode::Char(' ')]);
    assert_eq!(w.selected.iter().collect::<Vec<_>>(), ["openai/gpt"]);
    typ(&mut w, "-mini");
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert_eq!(st.visible().len(), 1);
    // Esc clears the filter first; `/` allows a leading `a`.
    press(&mut w, &[KeyCode::Esc]);
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert!(st.filter.is_empty());
    press(&mut w, &[KeyCode::Char('/')]);
    typ(&mut w, "anth ");
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert_eq!(st.filter, "anth ");
    assert!(st.filter_focus);
    press(&mut w, &[KeyCode::Backspace, KeyCode::Enter]);
    let ModelsPhase::List(st) = &w.models else {
        panic!()
    };
    assert!(!st.filter_focus);
    // ctrl+a selects all visible only.
    w.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(w.selected.contains("anthropic/claude-a"));
    assert!(draw(&w).contains("anthropic/claude-a"));
}

#[test]
fn fetch_error_offers_retry_and_manual_entry() {
    let env = Env::new(false);
    let mut w = env.wizard();
    let req = to_models(&mut w);
    // Stale results are ignored.
    w.models_loaded(req.generation + 7, Ok(models(&["x"])));
    assert_eq!(w.models, ModelsPhase::Loading);
    w.models_loaded(req.generation, Err("HTTP 401: invalid key".into()));
    assert!(draw(&w).contains("HTTP 401"));
    let Effect::FetchModels(again) = press(&mut w, &[KeyCode::Char('r')]) else {
        panic!("retry should refetch")
    };
    assert!(again.generation > req.generation);
    // The old generation can no longer land.
    w.models_loaded(req.generation, Ok(models(&["x"])));
    assert_eq!(w.models, ModelsPhase::Loading);
    w.models_loaded(again.generation, Err("boom".into()));
    press(&mut w, &[KeyCode::Char('m')]);
    assert!(matches!(w.models, ModelsPhase::Manual { .. }));
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("at least one"));
    typ(&mut w, " one , two,,one");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.chosen, ["one", "two"]);
    let ModelsPhase::Default { choices, .. } = &w.models else {
        panic!()
    };
    assert_eq!(choices, &["one", "two"]);
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.default_model, "one");
    assert_eq!(w.plan().profile.models, ["one", "two"]);
    // Esc from manual entry returns to the error options.
    let mut w2 = env.wizard();
    let req = to_models(&mut w2);
    w2.models_loaded(req.generation, Err("nope".into()));
    press(&mut w2, &[KeyCode::Down, KeyCode::Enter]);
    assert!(matches!(w2.models, ModelsPhase::Manual { .. }));
    press(&mut w2, &[KeyCode::Esc]);
    assert!(matches!(w2.models, ModelsPhase::Error { .. }));
}

#[test]
fn missing_modes_offers_init_or_skip() {
    let env = Env::new(false);
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(req.generation, Ok(models(&["only"])));
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]);
    assert_eq!(w.step, Step::Modes);
    assert!(draw(&w).contains("rc init"));
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::RunInit);
    assert!(draw(&w).contains("generating"));
    w.modes_initialized(Err("no templates".into()));
    assert!(draw(&w).contains("no templates"));
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::RunInit);
    w.modes_initialized(ModeSet::from_toml_str(MODES).map_err(|e| e.to_string()));
    assert!(matches!(w.modes_phase, ModesPhase::Assign { .. }));
    assert_eq!(w.assignments.len(), 2);

    // Skip path: no mode changes in the plan.
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(req.generation, Ok(models(&["only"])));
    press(
        &mut w,
        &[
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Enter,
        ],
    );
    assert_eq!(w.step, Step::Review);
    assert!(w.plan().mode_models.is_empty());
}

#[test]
fn existing_assignments_are_prefilled_and_clearable() {
    let env = Env::new(true);
    std::fs::write(
        env.proj.path().join(".ruckcode/modes.toml"),
        "[modes.explore]\nprofile = \"other\"\nmodel = \"fast\"\n[modes.test]\n",
    )
    .unwrap();
    std::fs::write(
        env.user.path().join("config.toml"),
        "[profiles.other]\nprovider = \"openai\"\nmodel = \"fast\"\nmodels = [\"fast\", \"slow\"]\n",
    )
    .unwrap();
    let mut w = env.wizard();
    let req = to_models(&mut w);
    w.models_loaded(req.generation, Ok(models(&["m"])));
    press(&mut w, &[KeyCode::Char(' '), KeyCode::Enter]);
    assert_eq!(w.assignments[0], Some(("other".into(), "fast".into())));
    // Options include the other profile's models.
    let opts = w.model_options();
    assert!(opts.contains(&("other".into(), "slow".into())));
    assert!(opts.contains(&("openrouter".into(), "m".into())));
    // Clear explore's assignment: chooser item 0.
    press(
        &mut w,
        &[KeyCode::Enter, KeyCode::Up, KeyCode::Up, KeyCode::Up],
    );
    // Filter in the chooser.
    typ(&mut w, "slow");
    assert!(draw(&w).contains("other/slow"));
    press(&mut w, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(w.assignments[0], Some(("other".into(), "slow".into())));
}

#[test]
fn profile_overwrite_prompt() {
    let env = Env::new(false);
    std::fs::write(
        env.user.path().join("config.toml"),
        "[profiles.openrouter]\nprovider = \"openai\"\nmodel = \"old\"\n",
    )
    .unwrap();
    let mut w = env.wizard();
    press(
        &mut w,
        &[
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    assert_eq!(w.step, Step::ProfileName);
    assert_eq!(press(&mut w, &[KeyCode::Enter]), Effect::None);
    assert!(w.confirm_overwrite);
    assert!(draw(&w).contains("already exists"));
    press(&mut w, &[KeyCode::Char('n')]);
    assert!(!w.confirm_overwrite);
    // Rename instead.
    typ(&mut w, "-2");
    assert!(matches!(
        press(&mut w, &[KeyCode::Enter]),
        Effect::FetchModels(_)
    ));
    // Invalid names are refused.
    let mut w = env.wizard();
    press(&mut w, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
    w.profile_name.set("bad name");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    assert!(w.error.is_some());
}

#[test]
fn auth_options_env_existing_none_and_scope() {
    let env = Env::new(false);
    ruckcode_core::keys::add_key(Some(env.user.path()), "acme", "sk-acme-0000", Some("Acme"))
        .unwrap();
    let mut w = env.wizard();
    // OpenAI preset: env var shown as set.
    press(&mut w, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
    assert_eq!(w.env_name.value, "OPENAI_API_KEY");
    assert!(w.env_is_set("OPENAI_API_KEY"));
    assert!(draw(&w).contains("currently set"));
    // Existing key + project scope.
    press(&mut w, &[KeyCode::Down]);
    assert_eq!(w.auth_choice(), Some(AuthChoice::Existing));
    assert!(draw(&w).contains("acme"));
    assert!(!draw(&w).contains("sk-acme-0000"));
    press(&mut w, &[KeyCode::Down, KeyCode::Down, KeyCode::Down]);
    assert_eq!(w.auth_cursor, AUTH_SCOPE_ROW);
    press(&mut w, &[KeyCode::Right]);
    assert_eq!(w.key_scope, KeyScope::Project);
    press(&mut w, &[KeyCode::Up, KeyCode::Up, KeyCode::Up]);
    assert_eq!(w.auth_choice(), Some(AuthChoice::Existing));
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::ProfileName);
    let p = w.plan();
    assert_eq!(p.profile.api_key_name.as_deref(), Some("acme"));
    assert_eq!(p.profile.api_key_env, None);
    assert_eq!(p.key_scope, KeyScope::Project);
    assert!(p.new_key.is_none());
    // Fetch uses the saved key's secret.
    let Effect::FetchModels(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert_eq!(r.api_key.as_deref(), Some("sk-acme-0000"));
    // No key at all (Ollama preset defaults to it).
    let mut w = env.wizard();
    press(
        &mut w,
        &[KeyCode::Down, KeyCode::Down, KeyCode::Enter, KeyCode::Enter],
    );
    assert_eq!(w.auth_choice(), Some(AuthChoice::NoKey));
    press(&mut w, &[KeyCode::Enter]);
    let Effect::FetchModels(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert_eq!(r.api_key, None);
    assert_eq!(
        w.plan().profile.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
}

#[test]
fn anthropic_default_endpoint_is_not_stored_and_custom_requires_url() {
    let env = Env::new(false);
    let mut w = env.wizard();
    assert_eq!(w.endpoint.value, "https://api.anthropic.com");
    w.default_model = "m".into();
    assert_eq!(w.plan().profile.base_url, None);
    let mut w = env.wizard();
    press(
        &mut w,
        &[
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Enter,
        ],
    );
    w.endpoint.set("localhost:8000");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Endpoint);
    assert!(w.error.is_some());
    w.endpoint.set("http://gpu-box:8000/v1");
    press(&mut w, &[KeyCode::Enter]);
    assert_eq!(w.step, Step::Auth);
    assert_eq!(w.profile_name.value, "gpu-box");
}

#[test]
fn ctrl_c_aborts_without_writing() {
    let env = Env::new(true);
    let mut w = env.wizard();
    to_models(&mut w);
    w.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(w.aborted && w.finished.is_none());
    assert!(!env.user.path().join("config.toml").exists());
}

#[test]
fn every_step_renders() {
    let env = Env::new(true);
    let mut w = env.wizard();
    let out = draw(&w);
    assert!(
        out.contains("step 1 of 7") && out.contains("Anthropic"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("step 2 of 7"));
    press(&mut w, &[KeyCode::Enter]);
    let out = draw(&w);
    assert!(
        out.contains("step 3 of 7") && out.contains("ANTHROPIC_API_KEY"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Down, KeyCode::Down]);
    assert!(draw(&w).contains("secret"));
    press(&mut w, &[KeyCode::Up, KeyCode::Up]);
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("step 4 of 7"));
    let Effect::FetchModels(r) = press(&mut w, &[KeyCode::Enter]) else {
        panic!()
    };
    assert!(draw(&w).contains("step 5 of 7"));
    w.models_loaded(r.generation, Ok(models(&["claude-a", "claude-b"])));
    assert!(draw(&w).contains("claude-b"));
    press(&mut w, &[KeyCode::Char('a'), KeyCode::Enter]);
    assert!(draw(&w).contains("default model"));
    press(&mut w, &[KeyCode::Enter]);
    let out = draw(&w);
    assert!(
        out.contains("step 6 of 7") && out.contains("explore"),
        "{out}"
    );
    press(&mut w, &[KeyCode::Enter]);
    assert!(draw(&w).contains("model for `explore`"));
    press(&mut w, &[KeyCode::Esc, KeyCode::Char('c')]);
    let out = draw(&w);
    assert!(
        out.contains("step 7 of 7") && out.contains("config.toml"),
        "{out}"
    );
    // Tiny terminals do not panic.
    let mut t = Terminal::new(TestBackend::new(20, 6)).unwrap();
    t.draw(|f| render::render(&w, f)).unwrap();
}
