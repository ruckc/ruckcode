//! Interactive terminal UI: an ACP client rendered with ratatui.

pub mod app;
pub mod render;

use std::io::{stdout, Stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ruckcode_acp::schema::{
    self, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, SessionUpdate,
};
use ruckcode_acp::{Connection, Handler, RpcError};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use self::app::{Action, App};
use crate::client::{self, AskRequest, AskResponse, SessionEntry, Started};
use crate::Cli;

/// Everything the UI loop reacts to besides keyboard input.
pub enum UiEvent {
    Update {
        session_id: String,
        update: SessionUpdate,
    },
    Permission {
        req: RequestPermissionRequest,
        reply: oneshot::Sender<RequestPermissionOutcome>,
    },
    Ask {
        req: AskRequest,
        reply: oneshot::Sender<AskResponse>,
    },
    TurnDone(Result<String, String>),
    Started(Started),
    ModeSet(String),
    Sessions(Vec<SessionEntry>),
    Error(String),
}

struct TuiHandler {
    tx: mpsc::UnboundedSender<UiEvent>,
}

#[async_trait]
impl Handler for TuiHandler {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            schema::SESSION_REQUEST_PERMISSION => {
                let req: RequestPermissionRequest = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let (reply, rx) = oneshot::channel();
                self.tx
                    .send(UiEvent::Permission { req, reply })
                    .map_err(|_| RpcError::internal("ui closed"))?;
                let outcome = rx.await.unwrap_or(RequestPermissionOutcome::Cancelled);
                Ok(serde_json::to_value(RequestPermissionResponse { outcome }).unwrap())
            }
            schema::ASK_USER_QUESTION => {
                let req: AskRequest = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let (reply, rx) = oneshot::channel();
                self.tx
                    .send(UiEvent::Ask { req, reply })
                    .map_err(|_| RpcError::internal("ui closed"))?;
                let resp = rx.await.unwrap_or_else(|_| AskResponse::cancelled());
                Ok(serde_json::to_value(resp).unwrap())
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }

    async fn notification(&self, method: &str, params: Value) {
        if method != schema::SESSION_UPDATE {
            return;
        }
        match serde_json::from_value::<SessionNotification>(params) {
            Ok(n) => {
                let _ = self.tx.send(UiEvent::Update {
                    session_id: n.session_id,
                    update: n.update,
                });
            }
            Err(e) => tracing::debug!("unparsed session/update: {e}"),
        }
    }
}

// ---- terminal -----------------------------------------------------------------------

type Term = Terminal<CrosstermBackend<Stdout>>;

fn restore_terminal() {
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<(Term, TerminalGuard)> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        if supports_keyboard_enhancement().unwrap_or(false) {
            let _ = execute!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            prev(info);
        }));
        Ok((Terminal::new(CrosstermBackend::new(out))?, TerminalGuard))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

// ---- spawned requests -------------------------------------------------------------------

struct Ctx {
    conn: Connection,
    tx: mpsc::UnboundedSender<UiEvent>,
    cwd: PathBuf,
    initial_mode: Option<String>,
}

impl Ctx {
    fn start(&self, load: Option<String>, use_mode: bool) {
        let (conn, tx, cwd) = (self.conn.clone(), self.tx.clone(), self.cwd.clone());
        let mode = if use_mode {
            self.initial_mode.clone()
        } else {
            None
        };
        tokio::spawn(async move {
            let r = async {
                let mut s = match load {
                    Some(id) => client::load_session(&conn, &cwd, &id).await?,
                    None => client::new_session(&conn, &cwd).await?,
                };
                if let Some(m) = mode {
                    client::set_mode(&conn, &s.session_id, &m).await?;
                    if let Some(st) = &mut s.modes {
                        st.current_mode_id = m;
                    }
                }
                anyhow::Ok(s)
            }
            .await;
            let _ = tx.send(match r {
                Ok(s) => UiEvent::Started(s),
                Err(e) => UiEvent::Error(format!("{e:#}")),
            });
        });
    }

    fn continue_last(&self) {
        let (conn, tx, cwd) = (self.conn.clone(), self.tx.clone(), self.cwd.clone());
        let mode = self.initial_mode.clone();
        tokio::spawn(async move {
            let r = async {
                let list = client::list_sessions(&conn, &cwd).await?;
                let Some(first) = list.first() else {
                    anyhow::bail!("no previous session to continue");
                };
                let mut s = client::load_session(&conn, &cwd, &first.id).await?;
                if let Some(m) = mode {
                    client::set_mode(&conn, &s.session_id, &m).await?;
                    if let Some(st) = &mut s.modes {
                        st.current_mode_id = m;
                    }
                }
                anyhow::Ok(s)
            }
            .await;
            let _ = tx.send(match r {
                Ok(s) => UiEvent::Started(s),
                Err(e) => UiEvent::Error(format!("{e:#}")),
            });
        });
    }

    fn list(&self) {
        let (conn, tx, cwd) = (self.conn.clone(), self.tx.clone(), self.cwd.clone());
        tokio::spawn(async move {
            let _ = tx.send(match client::list_sessions(&conn, &cwd).await {
                Ok(l) => UiEvent::Sessions(l),
                Err(e) => UiEvent::Error(format!("{e:#}")),
            });
        });
    }

    fn prompt(&self, sid: &str, text: String) {
        let (conn, tx, sid) = (self.conn.clone(), self.tx.clone(), sid.to_string());
        tokio::spawn(async move {
            let r: Result<PromptResponse, RpcError> = conn
                .request_typed(
                    schema::SESSION_PROMPT,
                    &schema::PromptRequest {
                        session_id: sid,
                        prompt: vec![schema::ContentBlock::text(text)],
                    },
                )
                .await;
            let _ = tx.send(UiEvent::TurnDone(match r {
                Ok(r) => Ok(serde_json::to_value(r.stop_reason)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()),
                Err(e) => Err(e.to_string()),
            }));
        });
    }

    fn set_mode(&self, sid: &str, mode: String) {
        let (conn, tx, sid) = (self.conn.clone(), self.tx.clone(), sid.to_string());
        tokio::spawn(async move {
            let _ = tx.send(match client::set_mode(&conn, &sid, &mode).await {
                Ok(()) => UiEvent::ModeSet(mode),
                Err(e) => UiEvent::Error(format!("{e:#}")),
            });
        });
    }

    fn cancel(&self, sid: &str) {
        let _ = self
            .conn
            .notify(schema::SESSION_CANCEL, json!({"sessionId": sid}));
    }
}

fn model_label(cli: &Cli, cwd: &Path) -> String {
    let cfg = ruckcode_core::Config::load(cwd, None).ok();
    let profile = cli
        .profile
        .clone()
        .or_else(|| cfg.as_ref().map(|c| c.default_profile.clone()))
        .unwrap_or_else(|| "default".into());
    let model = cli.model.clone().or_else(|| {
        cfg.as_ref()
            .and_then(|c| c.profiles.get(&profile))
            .map(|p| p.model.clone())
    });
    match model {
        Some(m) => format!("{profile}/{m}"),
        None => profile,
    }
}

// ---- main loop ------------------------------------------------------------------------------

pub async fn run(cli: &Cli) -> Result<()> {
    let cwd = cli.project_dir()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<UiEvent>();
    let conn = crate::connect(cli, Arc::new(TuiHandler { tx: tx.clone() })).await?;
    let ctx = Ctx {
        conn,
        tx,
        cwd: cwd.clone(),
        initial_mode: cli.mode.clone(),
    };

    let mut app = App::new(model_label(cli, &cwd));
    let mut initial_prompt = cli.prompt.clone();

    match cli.resume.as_deref() {
        Some("") => ctx.list(),
        Some(id) => {
            app.reset_for_session(Some(id.to_string()));
            ctx.start(Some(id.to_string()), true)
        }
        None if cli.continue_last => ctx.continue_last(),
        None => ctx.start(None, true),
    }

    let (mut term, _guard) = TerminalGuard::enter()?;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        term.draw(|f| render::render(&app, f))?;
        let mut action = Action::None;
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                    action = app.handle_key(k);
                }
                Some(Ok(Event::Mouse(m))) => app.handle_mouse(m),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            Some(ev) = rx.recv() => {
                action = handle_ui_event(&mut app, ev, &mut initial_prompt, &ctx);
            }
            _ = tick.tick() => app.tick = app.tick.wrapping_add(1),
        }
        run_action(&mut app, &ctx, action);
        if app.should_quit {
            break;
        }
    }
    if app.running {
        if let Some(sid) = &app.session_id {
            ctx.cancel(sid);
        }
    }
    Ok(())
}

fn handle_ui_event(
    app: &mut App,
    ev: UiEvent,
    initial_prompt: &mut Option<String>,
    ctx: &Ctx,
) -> Action {
    match ev {
        UiEvent::Update { session_id, update } => {
            if app.session_id.as_deref() == Some(session_id.as_str()) {
                app.handle_update(update);
            }
        }
        UiEvent::Permission { req, reply } => app.request_permission(req, reply),
        UiEvent::Ask { req, reply } => app.ask_questions(req, reply),
        UiEvent::TurnDone(r) => {
            match r {
                Ok(stop) => app.end_turn(None, Some(&stop)),
                Err(e) => app.end_turn(Some(e), None),
            }
            if let (Some(text), Some(sid)) = (app.next_queued(), app.session_id.clone()) {
                ctx.prompt(&sid, text);
            }
        }
        UiEvent::Started(s) => {
            let fresh = app.session_id.as_deref() != Some(s.session_id.as_str());
            if fresh {
                app.reset_for_session(Some(s.session_id.clone()));
            }
            let (modes, current) = client::mode_list(&s.modes);
            app.set_modes(modes, current);
            if let Some(text) = initial_prompt.take() {
                let text = app.begin_turn(text);
                ctx.prompt(&s.session_id, text);
            }
        }
        UiEvent::ModeSet(m) => app.mode_id = m,
        UiEvent::Sessions(l) => app.show_sessions(l),
        UiEvent::Error(e) => {
            // Startup failures leave nothing to talk to; keep the message visible.
            app.error(e);
            app.running = false;
        }
    }
    Action::None
}

fn run_action(app: &mut App, ctx: &Ctx, action: Action) {
    match action {
        Action::None | Action::Quit => {}
        Action::Submit(text) => match app.session_id.clone() {
            Some(sid) => ctx.prompt(&sid, text),
            None => {
                app.running = false;
                app.error("no active session yet");
            }
        },
        Action::Cancel => {
            if let Some(sid) = &app.session_id {
                ctx.cancel(sid);
            }
        }
        Action::SetMode(m) => match &app.session_id {
            Some(sid) => ctx.set_mode(sid, m),
            None => app.error("no active session yet"),
        },
        Action::NewSession => {
            app.reset_for_session(None);
            ctx.start(None, false);
        }
        Action::ListSessions => ctx.list(),
        Action::LoadSession(id) => {
            app.reset_for_session(Some(id.clone()));
            ctx.start(Some(id), false);
        }
    }
}
