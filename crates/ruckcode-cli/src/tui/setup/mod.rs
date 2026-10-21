//! The onboarding wizard: a pure state machine (`SetupWizard`), its renderer and the event loop
//! that drives it. All I/O (model fetch, `rc init`, writing files) happens through [`Effect`]s so
//! every transition can be tested without a terminal.

pub mod input;
pub mod render;
pub mod runner;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ruckcode_core::config::Profile;
use ruckcode_core::keys::{self, KeyInfo, KeyScope};
use ruckcode_core::setup::{self, NewKey, ProviderPreset, SetupPlan, SetupReport, PRESETS};
use ruckcode_core::{ModeSet, ProviderKind};
use ruckcode_llm::{ApiKind, ModelInfo};

pub use self::input::TextInput;

pub const TOTAL_STEPS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Provider,
    Endpoint,
    Auth,
    ProfileName,
    Models,
    Modes,
    Review,
}

impl Step {
    pub fn index(self) -> usize {
        self as usize + 1
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::Provider => "Provider",
            Step::Endpoint => "Endpoint",
            Step::Auth => "Authentication",
            Step::ProfileName => "Profile name",
            Step::Models => "Models",
            Step::Modes => "Models per mode",
            Step::Review => "Review & save",
        }
    }

    fn prev(self) -> Option<Step> {
        Some(match self {
            Step::Provider => return None,
            Step::Endpoint => Step::Provider,
            Step::Auth => Step::Endpoint,
            Step::ProfileName => Step::Auth,
            Step::Models => Step::ProfileName,
            Step::Modes => Step::Models,
            Step::Review => Step::Modes,
        })
    }
}

/// State of the project's `.ruckcode/modes.toml`.
pub enum ModesState {
    Missing,
    Broken(String),
    Ready(ModeSet),
}

/// Environment lookup (injectable for tests).
pub type EnvLookup = Box<dyn Fn(&str) -> Option<String>>;

pub struct SetupContext {
    pub project: PathBuf,
    pub user_dir: Option<PathBuf>,
    pub env: EnvLookup,
    /// Profiles defined in the user/project config files.
    pub existing_profiles: Vec<Profile>,
    /// Named keys in `credentials.toml`.
    pub keys: Vec<KeyInfo>,
    pub modes: ModesState,
}

impl SetupContext {
    /// Read the current state from disk and the process environment.
    pub fn load(project: &Path, user_dir: Option<&Path>) -> Self {
        let path = project.join(".ruckcode/modes.toml");
        let modes = match std::fs::read_to_string(&path) {
            Err(_) => ModesState::Missing,
            Ok(t) => match ModeSet::from_toml_str(&t) {
                Ok(m) => ModesState::Ready(m),
                Err(e) => ModesState::Broken(format!("{e:#}")),
            },
        };
        Self {
            project: project.to_path_buf(),
            user_dir: user_dir.map(Path::to_path_buf),
            env: Box::new(setup::process_env),
            existing_profiles: setup::configured_profiles(project, user_dir),
            keys: keys::list_keys(user_dir).unwrap_or_default(),
            modes,
        }
    }
}

/// Work the event loop must do on the wizard's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    FetchModels(FetchRequest),
    /// Generate `.ruckcode/modes.toml` (`rc init`).
    RunInit,
    /// Write everything (`setup::apply` with [`SetupWizard::plan`]).
    Apply,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchRequest {
    pub generation: u64,
    pub kind: ApiKind,
    pub base_url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthChoice {
    Env,
    Existing,
    New,
    NoKey,
}

pub const AUTH_CHOICES: [AuthChoice; 4] = [
    AuthChoice::Env,
    AuthChoice::Existing,
    AuthChoice::New,
    AuthChoice::NoKey,
];
/// Cursor row of the key-scope toggle (after the four choices).
pub const AUTH_SCOPE_ROW: usize = 4;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListState {
    pub items: Vec<ModelInfo>,
    pub cursor: usize,
    pub filter: String,
    /// `/` focuses the filter so every character (including `a` and space) is typed into it.
    pub filter_focus: bool,
}

impl ListState {
    /// Indices of `items` matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                f.is_empty()
                    || m.id.to_lowercase().contains(&f)
                    || m.display_name
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(&f))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelsPhase {
    Loading,
    Error {
        msg: String,
        cursor: usize,
    },
    Manual {
        input: TextInput,
        err: Option<String>,
    },
    List(ListState),
    Default {
        choices: Vec<String>,
        cursor: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chooser {
    pub cursor: usize,
    pub filter: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModesPhase {
    NeedInit {
        cursor: usize,
        error: Option<String>,
    },
    Initing,
    Broken,
    Assign {
        row: usize,
        chooser: Option<Chooser>,
    },
}

pub type ModelChoice = (String, String);

pub struct SetupWizard {
    pub ctx: SetupContext,
    pub step: Step,
    pub error: Option<String>,
    pub tick: u64,
    pub aborted: bool,
    pub finished: Option<SetupReport>,

    // 1-2
    pub preset_cursor: usize,
    pub preset_idx: usize,
    pub endpoint: TextInput,
    // 3
    pub auth_cursor: usize,
    pub env_name: TextInput,
    pub existing_idx: usize,
    pub new_key_name: TextInput,
    pub new_key_secret: TextInput,
    /// 0 = name, 1 = secret.
    pub new_field: usize,
    pub key_scope: KeyScope,
    // 4
    pub profile_name: TextInput,
    pub confirm_overwrite: bool,
    // 5
    pub models: ModelsPhase,
    pub generation: u64,
    fetched_for: Option<(ApiKind, String, Option<String>)>,
    fetched: Vec<ModelInfo>,
    last_fetch_error: Option<String>,
    pub selected: BTreeSet<String>,
    pub chosen: Vec<String>,
    pub default_model: String,
    // 6
    pub modes_phase: ModesPhase,
    pub assignments: Vec<Option<ModelChoice>>,
    assignments_ready: bool,
    pub modes_skipped: bool,
    // 7
    pub set_default: bool,
    pub preview: String,
    pub preview_scroll: usize,
    pub apply_error: Option<String>,
}

fn is_ctrl_c(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c')
}

fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_profile_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Move a list cursor with the arrow keys (and j/k when `vi`).
fn nav(cursor: &mut usize, len: usize, key: &KeyEvent, vi: bool) -> bool {
    if len == 0 {
        return false;
    }
    match key.code {
        KeyCode::Up => *cursor = (*cursor + len - 1) % len,
        KeyCode::Down => *cursor = (*cursor + 1) % len,
        KeyCode::Char('k') if vi => *cursor = (*cursor + len - 1) % len,
        KeyCode::Char('j') if vi => *cursor = (*cursor + 1) % len,
        _ => return false,
    }
    true
}

/// A default profile name for a custom endpoint, from its host.
pub fn derive_profile_name(preset: &ProviderPreset, endpoint: &str) -> String {
    if preset.base_url.is_some() {
        return preset.id.to_string();
    }
    let host = endpoint
        .split("://")
        .nth(1)
        .unwrap_or(endpoint)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    let name: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "custom".into()
    } else {
        name
    }
}

impl SetupWizard {
    pub fn new(ctx: SetupContext) -> Self {
        let mut w = Self {
            ctx,
            step: Step::Provider,
            error: None,
            tick: 0,
            aborted: false,
            finished: None,
            preset_cursor: 0,
            preset_idx: usize::MAX,
            endpoint: TextInput::default(),
            auth_cursor: 0,
            env_name: TextInput::default(),
            existing_idx: 0,
            new_key_name: TextInput::default(),
            new_key_secret: TextInput::default(),
            new_field: 0,
            key_scope: KeyScope::User,
            profile_name: TextInput::default(),
            confirm_overwrite: false,
            models: ModelsPhase::Loading,
            generation: 0,
            fetched_for: None,
            fetched: vec![],
            last_fetch_error: None,
            selected: BTreeSet::new(),
            chosen: vec![],
            default_model: String::new(),
            modes_phase: ModesPhase::Initing,
            assignments: vec![],
            assignments_ready: false,
            modes_skipped: false,
            set_default: true,
            preview: String::new(),
            preview_scroll: 0,
            apply_error: None,
        };
        w.apply_preset(0);
        w
    }

    pub fn preset(&self) -> &'static ProviderPreset {
        &PRESETS[self.preset_idx]
    }

    pub fn kind(&self) -> ProviderKind {
        self.preset().kind
    }

    pub fn env_is_set(&self, name: &str) -> bool {
        (self.ctx.env)(name).is_some_and(|v| !v.is_empty())
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn done(&self) -> bool {
        self.aborted || self.finished.is_some()
    }

    fn apply_preset(&mut self, idx: usize) {
        if self.preset_idx == idx {
            return;
        }
        self.preset_idx = idx;
        self.preset_cursor = idx;
        let p = &PRESETS[idx];
        self.endpoint
            .set(p.base_url.unwrap_or("http://localhost:8000/v1"));
        self.env_name.set(
            &p.api_key_env
                .map(String::from)
                .unwrap_or_else(|| format!("{}_API_KEY", p.id.to_uppercase())),
        );
        self.auth_cursor = if p.api_key_env.is_some() { 0 } else { 3 };
        self.new_key_name.set(p.id);
        self.new_key_secret.set("");
        self.profile_name
            .set(&derive_profile_name(p, p.base_url.unwrap_or("")));
        self.fetched_for = None;
    }

    // ---- key handling ------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if is_ctrl_c(&key) {
            self.aborted = true;
            return Effect::None;
        }
        self.error = None;
        match self.step {
            Step::Provider => self.key_provider(key),
            Step::Endpoint => self.key_endpoint(key),
            Step::Auth => self.key_auth(key),
            Step::ProfileName => self.key_profile_name(key),
            Step::Models => self.key_models(key),
            Step::Modes => self.key_modes(key),
            Step::Review => self.key_review(key),
        }
    }

    fn back(&mut self) {
        self.error = None;
        match self.step.prev() {
            Some(s) => self.step = s,
            None => self.aborted = true,
        }
    }

    fn key_provider(&mut self, key: KeyEvent) -> Effect {
        if nav(&mut self.preset_cursor, PRESETS.len(), &key, true) {
            return Effect::None;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => {
                self.apply_preset(self.preset_cursor);
                self.step = Step::Endpoint;
            }
            _ => {}
        }
        Effect::None
    }

    fn key_endpoint(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => {
                let u = self.endpoint.trimmed();
                if !(u.starts_with("http://") || u.starts_with("https://")) {
                    self.error = Some("the endpoint must start with http:// or https://".into());
                } else {
                    if self.profile_name.trimmed().is_empty()
                        || self.profile_name.trimmed() == derive_profile_name(self.preset(), "")
                    {
                        let n = derive_profile_name(self.preset(), u);
                        self.profile_name.set(&n);
                    }
                    self.step = Step::Auth;
                }
            }
            _ => {
                self.endpoint.handle(key);
            }
        }
        Effect::None
    }

    pub fn auth_choice(&self) -> Option<AuthChoice> {
        AUTH_CHOICES.get(self.auth_cursor).copied()
    }

    fn key_auth(&mut self, key: KeyEvent) -> Effect {
        let rows = AUTH_SCOPE_ROW + 1;
        if nav(&mut self.auth_cursor, rows, &key, false) {
            return Effect::None;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => self.confirm_auth(),
            _ => match self.auth_cursor {
                0 => {
                    self.env_name.handle(key);
                }
                1 => match key.code {
                    KeyCode::Left if !self.ctx.keys.is_empty() => {
                        let n = self.ctx.keys.len();
                        self.existing_idx = (self.existing_idx + n - 1) % n;
                    }
                    KeyCode::Right if !self.ctx.keys.is_empty() => {
                        self.existing_idx = (self.existing_idx + 1) % self.ctx.keys.len();
                    }
                    _ => {}
                },
                2 => match key.code {
                    KeyCode::Tab | KeyCode::BackTab => self.new_field = 1 - self.new_field,
                    _ => {
                        if self.new_field == 0 {
                            self.new_key_name.handle(key);
                        } else {
                            self.new_key_secret.handle(key);
                        }
                    }
                },
                AUTH_SCOPE_ROW => {
                    if matches!(
                        key.code,
                        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                    ) {
                        self.key_scope = match self.key_scope {
                            KeyScope::User => KeyScope::Project,
                            KeyScope::Project => KeyScope::User,
                        };
                    }
                }
                _ => {}
            },
        }
        Effect::None
    }

    fn confirm_auth(&mut self) {
        match self.auth_choice() {
            Some(AuthChoice::Env) if !is_env_name(self.env_name.trimmed()) => {
                self.error = Some("enter an environment variable name, e.g. MY_API_KEY".into());
                return;
            }
            Some(AuthChoice::Existing) if self.ctx.keys.is_empty() => {
                self.error = Some("no named keys saved yet; add a new one".into());
                return;
            }
            Some(AuthChoice::New) => {
                if let Err(e) = keys::validate_key_name(self.new_key_name.trimmed()) {
                    self.error = Some(e.to_string());
                    return;
                }
                if self.new_key_secret.trimmed().is_empty() {
                    self.error = Some("paste the API key".into());
                    self.new_field = 1;
                    return;
                }
            }
            None => {
                // The cursor is on the scope row: nothing chosen there, pick a sensible row.
                self.error = Some("choose how to authenticate (move up)".into());
                return;
            }
            _ => {}
        }
        self.step = Step::ProfileName;
        self.confirm_overwrite = false;
    }

    fn profile_exists(&self, name: &str) -> bool {
        self.ctx.existing_profiles.iter().any(|p| p.name == name)
    }

    fn key_profile_name(&mut self, key: KeyEvent) -> Effect {
        if self.confirm_overwrite {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm_overwrite = false;
                    return self.enter_models();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_overwrite = false;
                }
                _ => {}
            }
            return Effect::None;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => {
                let n = self.profile_name.trimmed().to_string();
                if !is_profile_name(&n) {
                    self.error =
                        Some("use letters, digits, `-` or `_` for the profile name".into());
                } else if self.profile_exists(&n) {
                    self.confirm_overwrite = true;
                } else {
                    return self.enter_models();
                }
            }
            _ => {
                self.profile_name.handle(key);
            }
        }
        Effect::None
    }

    // ---- models --------------------------------------------------------------------------

    /// The key used to query the endpoint, if any.
    pub fn fetch_key(&self) -> Option<String> {
        match self.auth_choice() {
            Some(AuthChoice::Env) => {
                (self.ctx.env)(self.env_name.trimmed()).filter(|k| !k.is_empty())
            }
            Some(AuthChoice::Existing) => self.ctx.keys.get(self.existing_idx).and_then(|k| {
                ruckcode_core::config::named_key(self.ctx.user_dir.as_deref(), &k.name)
            }),
            Some(AuthChoice::New) => Some(self.new_key_secret.trimmed().to_string()),
            _ => None,
        }
    }

    fn api_kind(&self) -> ApiKind {
        match self.kind() {
            ProviderKind::Anthropic => ApiKind::Anthropic,
            _ => ApiKind::Openai,
        }
    }

    fn enter_models(&mut self) -> Effect {
        self.step = Step::Models;
        let base = self.endpoint.trimmed().trim_end_matches('/').to_string();
        let key = self.fetch_key();
        let signature = (self.api_kind(), base.clone(), key.clone());
        if self.fetched_for.as_ref() == Some(&signature) && !self.fetched.is_empty() {
            self.models = ModelsPhase::List(ListState {
                items: self.fetched.clone(),
                ..Default::default()
            });
            return Effect::None;
        }
        self.generation += 1;
        self.models = ModelsPhase::Loading;
        Effect::FetchModels(FetchRequest {
            generation: self.generation,
            kind: signature.0,
            base_url: base,
            api_key: key,
        })
    }

    /// Result of a [`Effect::FetchModels`]; stale generations are ignored.
    pub fn models_loaded(&mut self, generation: u64, r: Result<Vec<ModelInfo>, String>) {
        if generation != self.generation
            || self.step != Step::Models
            || self.models != ModelsPhase::Loading
        {
            return;
        }
        match r {
            Ok(items) if !items.is_empty() => {
                self.fetched_for = Some((
                    self.api_kind(),
                    self.endpoint.trimmed().trim_end_matches('/').to_string(),
                    self.fetch_key(),
                ));
                self.fetched = items.clone();
                self.last_fetch_error = None;
                self.models = ModelsPhase::List(ListState {
                    items,
                    ..Default::default()
                });
            }
            other => {
                let msg = match other {
                    Err(e) => e,
                    _ => "the endpoint returned no models".into(),
                };
                self.last_fetch_error = Some(msg.clone());
                self.models = ModelsPhase::Error { msg, cursor: 0 };
            }
        }
    }

    fn key_models(&mut self, key: KeyEvent) -> Effect {
        match self.models.clone() {
            ModelsPhase::Loading => {
                if key.code == KeyCode::Esc {
                    self.generation += 1;
                    self.back();
                }
                Effect::None
            }
            ModelsPhase::Error { msg, mut cursor } => {
                if nav(&mut cursor, 2, &key, true) {
                    self.models = ModelsPhase::Error { msg, cursor };
                    return Effect::None;
                }
                match key.code {
                    KeyCode::Esc => self.back(),
                    KeyCode::Char('r') => return self.retry_fetch(),
                    KeyCode::Char('m') => self.open_manual(),
                    KeyCode::Enter if cursor == 0 => return self.retry_fetch(),
                    KeyCode::Enter => self.open_manual(),
                    _ => {}
                }
                Effect::None
            }
            ModelsPhase::Manual { mut input, .. } => {
                match key.code {
                    KeyCode::Esc => {
                        self.models = ModelsPhase::Error {
                            msg: self
                                .last_fetch_error
                                .clone()
                                .unwrap_or_else(|| "model list not fetched".into()),
                            cursor: 1,
                        };
                    }
                    KeyCode::Enter => {
                        let ids = parse_model_ids(&input.value);
                        if ids.is_empty() {
                            self.models = ModelsPhase::Manual {
                                input,
                                err: Some("enter at least one model id".into()),
                            };
                        } else {
                            self.selected = ids.iter().cloned().collect();
                            self.finish_models(ids);
                        }
                    }
                    _ => {
                        input.handle(key);
                        self.models = ModelsPhase::Manual { input, err: None };
                    }
                }
                Effect::None
            }
            ModelsPhase::List(st) => self.key_model_list(st, key),
            ModelsPhase::Default {
                choices,
                mut cursor,
            } => {
                if nav(&mut cursor, choices.len(), &key, true) {
                    self.models = ModelsPhase::Default { choices, cursor };
                    return Effect::None;
                }
                match key.code {
                    KeyCode::Esc => {
                        self.models = if self.fetched.is_empty() {
                            ModelsPhase::Manual {
                                input: TextInput::new(self.chosen.join(", ")),
                                err: None,
                            }
                        } else {
                            ModelsPhase::List(ListState {
                                items: self.fetched.clone(),
                                ..Default::default()
                            })
                        };
                    }
                    KeyCode::Enter => {
                        self.default_model = choices[cursor].clone();
                        self.enter_modes();
                    }
                    _ => {}
                }
                Effect::None
            }
        }
    }

    fn retry_fetch(&mut self) -> Effect {
        self.fetched_for = None;
        self.enter_models()
    }

    fn open_manual(&mut self) {
        self.models = ModelsPhase::Manual {
            input: TextInput::new(self.chosen.join(", ")),
            err: None,
        };
    }

    fn key_model_list(&mut self, mut st: ListState, key: KeyEvent) -> Effect {
        let vis = st.visible();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if st.filter_focus {
                    st.filter_focus = false;
                } else if !st.filter.is_empty() {
                    st.filter.clear();
                    st.cursor = 0;
                } else {
                    self.back();
                    return Effect::None;
                }
            }
            KeyCode::Enter => {
                if st.filter_focus {
                    st.filter_focus = false;
                } else if self.selected.is_empty() {
                    self.error = Some("select at least one model with Space".into());
                } else {
                    let ids: Vec<String> = st
                        .items
                        .iter()
                        .filter(|m| self.selected.contains(&m.id))
                        .map(|m| m.id.clone())
                        .collect();
                    self.models = ModelsPhase::List(st);
                    self.finish_models(ids);
                    return Effect::None;
                }
            }
            KeyCode::Up | KeyCode::Down => {
                nav(&mut st.cursor, vis.len(), &key, false);
            }
            KeyCode::PageUp => st.cursor = st.cursor.saturating_sub(10),
            KeyCode::PageDown => st.cursor = (st.cursor + 10).min(vis.len().saturating_sub(1)),
            KeyCode::Char('a') if ctrl => self.toggle_all(&st),
            KeyCode::Char(' ') if !st.filter_focus => {
                if let Some(&i) = vis.get(st.cursor) {
                    let id = st.items[i].id.clone();
                    if !self.selected.remove(&id) {
                        self.selected.insert(id);
                    }
                }
            }
            KeyCode::Char('/') if !st.filter_focus && st.filter.is_empty() => {
                st.filter_focus = true;
            }
            KeyCode::Char('a') if !st.filter_focus && st.filter.is_empty() => self.toggle_all(&st),
            KeyCode::Char(c) if !ctrl => {
                st.filter.push(c);
                st.cursor = 0;
            }
            KeyCode::Backspace => {
                st.filter.pop();
                st.cursor = 0;
            }
            _ => {}
        }
        let n = st.visible().len();
        if st.cursor >= n {
            st.cursor = n.saturating_sub(1);
        }
        self.models = ModelsPhase::List(st);
        Effect::None
    }

    /// Select every visible model, or deselect them if all are already selected.
    fn toggle_all(&mut self, st: &ListState) {
        let ids: Vec<String> = st
            .visible()
            .iter()
            .map(|&i| st.items[i].id.clone())
            .collect();
        if ids.iter().all(|i| self.selected.contains(i)) {
            for i in ids {
                self.selected.remove(&i);
            }
        } else {
            self.selected.extend(ids);
        }
    }

    fn finish_models(&mut self, ids: Vec<String>) {
        self.chosen = ids.clone();
        if ids.len() == 1 {
            self.default_model = ids[0].clone();
            self.enter_modes();
            return;
        }
        let existing = self
            .ctx
            .existing_profiles
            .iter()
            .find(|p| p.name == self.profile_name.trimmed())
            .map(|p| p.model.clone());
        let cursor = existing
            .and_then(|m| ids.iter().position(|i| *i == m))
            .unwrap_or(0);
        self.models = ModelsPhase::Default {
            choices: ids,
            cursor,
        };
    }

    // ---- per-mode assignment ---------------------------------------------------------------

    pub fn mode_set(&self) -> Option<&ModeSet> {
        match &self.ctx.modes {
            ModesState::Ready(m) => Some(m),
            _ => None,
        }
    }

    /// Every (profile, model) a mode may use: the new profile's models, then other profiles'.
    pub fn model_options(&self) -> Vec<ModelChoice> {
        let name = self.profile_name.trimmed().to_string();
        let mut out: Vec<ModelChoice> = vec![];
        let mut push = |p: &str, m: &str| {
            let c = (p.to_string(), m.to_string());
            if !m.is_empty() && !out.contains(&c) {
                out.push(c);
            }
        };
        for m in &self.chosen {
            push(&name, m);
        }
        for p in self.ctx.existing_profiles.iter().filter(|p| p.name != name) {
            push(&p.name, &p.model);
            for m in &p.models {
                push(&p.name, m);
            }
        }
        // Keep assignments that already exist even if their profile isn't listed above.
        for a in self.assignments.iter().flatten() {
            push(&a.0, &a.1);
        }
        out
    }

    fn enter_modes(&mut self) {
        self.step = Step::Modes;
        match &self.ctx.modes {
            ModesState::Missing => {
                if !matches!(self.modes_phase, ModesPhase::NeedInit { .. }) {
                    self.modes_phase = ModesPhase::NeedInit {
                        cursor: 0,
                        error: None,
                    };
                }
            }
            ModesState::Broken(_) => self.modes_phase = ModesPhase::Broken,
            ModesState::Ready(set) => {
                let n = set.modes.len();
                if !self.assignments_ready {
                    self.assignments = set
                        .modes
                        .iter()
                        .map(|(_, m)| self.existing_assignment(m))
                        .collect();
                    self.assignments_ready = true;
                }
                self.assignments.resize(n, None);
                // Drop assignments to models that were just deselected from this profile.
                let name = self.profile_name.trimmed().to_string();
                let chosen = self.chosen.clone();
                for a in &mut self.assignments {
                    if a.as_ref()
                        .is_some_and(|(p, m)| *p == name && !chosen.contains(m))
                    {
                        *a = None;
                    }
                }
                self.modes_skipped = false;
                if !matches!(self.modes_phase, ModesPhase::Assign { .. }) {
                    self.modes_phase = ModesPhase::Assign {
                        row: 0,
                        chooser: None,
                    };
                }
            }
        }
    }

    fn existing_assignment(&self, m: &ruckcode_core::ModeDef) -> Option<ModelChoice> {
        let profile = m.profile.clone()?;
        let model = m.model.clone().or_else(|| {
            self.ctx
                .existing_profiles
                .iter()
                .find(|p| p.name == profile)
                .map(|p| p.model.clone())
        })?;
        Some((profile, model))
    }

    /// Items of the chooser: "profile default" followed by the options matching `filter`.
    pub fn chooser_items(&self, filter: &str) -> Vec<Option<ModelChoice>> {
        let f = filter.to_lowercase();
        let mut v = vec![None];
        v.extend(
            self.model_options()
                .into_iter()
                .filter(|(p, m)| f.is_empty() || format!("{p}/{m}").to_lowercase().contains(&f))
                .map(Some),
        );
        v
    }

    /// Result of [`Effect::RunInit`].
    pub fn modes_initialized(&mut self, r: Result<ModeSet, String>) {
        if self.step != Step::Modes || self.modes_phase != ModesPhase::Initing {
            return;
        }
        match r {
            Ok(set) => {
                self.ctx.modes = ModesState::Ready(set);
                self.modes_phase = ModesPhase::Assign {
                    row: 0,
                    chooser: None,
                };
                self.assignments_ready = false;
                self.enter_modes();
            }
            Err(e) => {
                self.modes_phase = ModesPhase::NeedInit {
                    cursor: 0,
                    error: Some(e),
                }
            }
        }
    }

    fn key_modes(&mut self, key: KeyEvent) -> Effect {
        match self.modes_phase.clone() {
            ModesPhase::Initing => {
                if key.code == KeyCode::Esc {
                    self.back();
                }
                Effect::None
            }
            ModesPhase::NeedInit { mut cursor, error } => {
                if nav(&mut cursor, 2, &key, true) {
                    self.modes_phase = ModesPhase::NeedInit { cursor, error };
                    return Effect::None;
                }
                match key.code {
                    KeyCode::Esc => self.back(),
                    KeyCode::Enter if cursor == 0 => {
                        self.modes_phase = ModesPhase::Initing;
                        return Effect::RunInit;
                    }
                    KeyCode::Enter => {
                        self.modes_skipped = true;
                        return self.enter_review();
                    }
                    _ => {}
                }
                Effect::None
            }
            ModesPhase::Broken => {
                match key.code {
                    KeyCode::Esc => self.back(),
                    KeyCode::Enter => {
                        self.modes_skipped = true;
                        return self.enter_review();
                    }
                    _ => {}
                }
                Effect::None
            }
            ModesPhase::Assign { mut row, chooser } => {
                let n = self.mode_set().map_or(0, |m| m.modes.len());
                match chooser {
                    Some(ch) => self.key_chooser(row, ch, key),
                    None => {
                        if nav(&mut row, n + 1, &key, true) {
                            self.modes_phase = ModesPhase::Assign { row, chooser: None };
                            return Effect::None;
                        }
                        match key.code {
                            KeyCode::Esc => self.back(),
                            KeyCode::Enter if row == n => return self.enter_review(),
                            KeyCode::Enter => {
                                let cur = self.assignments.get(row).cloned().flatten();
                                let items = self.chooser_items("");
                                let cursor = items.iter().position(|i| *i == cur).unwrap_or(0);
                                self.modes_phase = ModesPhase::Assign {
                                    row,
                                    chooser: Some(Chooser {
                                        cursor,
                                        filter: String::new(),
                                    }),
                                };
                            }
                            KeyCode::Char('c') => return self.enter_review(),
                            _ => {}
                        }
                        Effect::None
                    }
                }
            }
        }
    }

    fn key_chooser(&mut self, row: usize, mut ch: Chooser, key: KeyEvent) -> Effect {
        let items = self.chooser_items(&ch.filter);
        match key.code {
            KeyCode::Esc => {
                self.modes_phase = ModesPhase::Assign { row, chooser: None };
                return Effect::None;
            }
            KeyCode::Enter => {
                if let Some(choice) = items.get(ch.cursor) {
                    if let Some(slot) = self.assignments.get_mut(row) {
                        *slot = choice.clone();
                    }
                }
                self.modes_phase = ModesPhase::Assign { row, chooser: None };
                return Effect::None;
            }
            KeyCode::Up | KeyCode::Down => {
                nav(&mut ch.cursor, items.len(), &key, false);
            }
            KeyCode::PageUp => ch.cursor = ch.cursor.saturating_sub(10),
            KeyCode::PageDown => ch.cursor = (ch.cursor + 10).min(items.len() - 1),
            KeyCode::Backspace => {
                ch.filter.pop();
                ch.cursor = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                ch.filter.push(c);
                ch.cursor = 0;
            }
            _ => {}
        }
        self.modes_phase = ModesPhase::Assign {
            row,
            chooser: Some(ch),
        };
        Effect::None
    }

    // ---- review ----------------------------------------------------------------------------

    /// The plan the wizard currently describes.
    pub fn plan(&self) -> SetupPlan {
        let preset = self.preset();
        let name = self.profile_name.trimmed().to_string();
        let (mut api_key_env, mut api_key_name, mut new_key) = (None, None, None);
        match self.auth_choice() {
            Some(AuthChoice::Env) => api_key_env = Some(self.env_name.trimmed().to_string()),
            Some(AuthChoice::Existing) => {
                api_key_name = self.ctx.keys.get(self.existing_idx).map(|k| k.name.clone())
            }
            Some(AuthChoice::New) => {
                let n = self.new_key_name.trimmed().to_string();
                api_key_name = Some(n.clone());
                new_key = Some(NewKey {
                    name: n,
                    secret: self.new_key_secret.trimmed().to_string(),
                    description: Some(format!("{} key", preset.label)),
                });
            }
            _ => {}
        }
        let mode_models = match self.mode_set() {
            Some(set) if !self.modes_skipped => set
                .modes
                .iter()
                .enumerate()
                .map(|(i, (id, _))| (id.clone(), self.assignments.get(i).cloned().flatten()))
                .collect(),
            _ => vec![],
        };
        SetupPlan {
            profile_name: name.clone(),
            profile: Profile {
                name,
                provider: preset.kind,
                model: self.default_model.clone(),
                models: self.chosen.clone(),
                api_key_name,
                api_key_env,
                base_url: setup::stored_base_url(preset.kind, self.endpoint.trimmed()),
                max_tokens: 32_000,
                context_window: 200_000,
                thinking: None,
            },
            set_default: self.set_default,
            new_key,
            key_scope: self.key_scope,
            mode_models,
        }
    }

    fn enter_review(&mut self) -> Effect {
        self.step = Step::Review;
        self.apply_error = None;
        self.preview_scroll = 0;
        self.refresh_preview();
        Effect::None
    }

    fn refresh_preview(&mut self) {
        self.preview = match setup::preview(
            &self.plan(),
            self.ctx.user_dir.as_deref(),
            &self.ctx.project,
        ) {
            Ok(p) => p,
            Err(e) => format!("cannot preview: {e:#}"),
        };
    }

    fn key_review(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
            KeyCode::Down => self.preview_scroll += 1,
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            KeyCode::PageDown => self.preview_scroll += 10,
            KeyCode::Char('d') => {
                self.set_default = !self.set_default;
                self.refresh_preview();
            }
            KeyCode::Enter => return Effect::Apply,
            _ => {}
        }
        Effect::None
    }

    /// Result of [`Effect::Apply`].
    pub fn applied(&mut self, r: Result<SetupReport, String>) {
        match r {
            Ok(report) => self.finished = Some(report),
            Err(e) => self.apply_error = Some(e),
        }
    }
}

/// Comma/whitespace-separated model ids, de-duplicated in order.
pub fn parse_model_ids(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in s
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests;
