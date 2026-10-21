//! Drives a [`SetupWizard`] on a real terminal and performs its effects.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ruckcode_core::init::{self, InitOptions};
use ruckcode_core::setup::{self, SetupReport};
use ruckcode_core::ModeSet;
use ruckcode_llm::ModelInfo;
use tokio::sync::mpsc;

use super::{Effect, FetchRequest, SetupContext, SetupWizard};
use crate::tui::{Term, TerminalGuard};
use crate::Cli;

#[derive(Debug)]
pub enum Outcome {
    Finished(SetupReport),
    Aborted,
}

enum Msg {
    Models(u64, Result<Vec<ModelInfo>, String>),
    Init(Result<ModeSet, String>),
}

fn spawn_fetch(req: FetchRequest, tx: mpsc::UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let r = ruckcode_llm::list_models(req.kind, &req.base_url, req.api_key.as_deref())
            .await
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(Msg::Models(req.generation, r));
    });
}

/// `rc init` for the wizard: generate modes.toml from templates and discovered mise tasks.
async fn init_modes(project: std::path::PathBuf) -> Result<ModeSet, String> {
    let tasks = ruckcode_mise::discover(&project).await;
    let text = init::generate(&tasks, &InitOptions::default()).map_err(|e| format!("{e:#}"))?;
    let path = project.join(".ruckcode").join("modes.toml");
    std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    std::fs::write(&path, &text).map_err(|e| e.to_string())?;
    ModeSet::from_toml_str(&text).map_err(|e| format!("{e:#}"))
}

/// Run the wizard to completion on `term`, reading keys from `events`.
pub async fn run_wizard(
    term: &mut Term,
    events: &mut EventStream,
    mut w: SetupWizard,
) -> Result<Outcome> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    while !w.done() {
        term.draw(|f| super::render::render(&w, f))?;
        let mut effect = Effect::None;
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                    effect = w.handle_key(k);
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return Ok(Outcome::Aborted),
            },
            Some(m) = rx.recv() => match m {
                Msg::Models(g, r) => w.models_loaded(g, r),
                Msg::Init(r) => w.modes_initialized(r),
            },
            _ = tick.tick() => w.tick(),
        }
        match effect {
            Effect::None => {}
            Effect::FetchModels(req) => spawn_fetch(req, tx.clone()),
            Effect::RunInit => {
                let (tx, project) = (tx.clone(), w.ctx.project.clone());
                tokio::spawn(async move {
                    let _ = tx.send(Msg::Init(init_modes(project).await));
                });
            }
            Effect::Apply => {
                let r = setup::apply(&w.plan(), w.ctx.user_dir.as_deref(), &w.ctx.project)
                    .map_err(|e| format!("{e:#}"));
                w.applied(r);
            }
        }
    }
    Ok(match w.finished.take() {
        Some(r) => Outcome::Finished(r),
        None => Outcome::Aborted,
    })
}

/// Human-readable summary of what was written.
pub fn summary(r: &SetupReport) -> String {
    let mut s = format!("saved profile settings to {}", r.config_path.display());
    if let Some(p) = &r.project_config_path {
        s.push_str(&format!(
            "\nsaved key reference to {} (this project only)",
            p.display()
        ));
    }
    if let Some(p) = &r.credentials_path {
        s.push_str(&format!("\nsaved API key to {} (mode 0600)", p.display()));
    }
    if let Some(p) = &r.modes_path {
        s.push_str(&format!("\nupdated per-mode models in {}", p.display()));
    }
    s
}

/// `rc setup`: the wizard on its own terminal session.
pub async fn standalone(cli: &Cli) -> Result<()> {
    let project = cli.project_dir()?;
    let w = SetupWizard::new(SetupContext::load(&project, None));
    let outcome = {
        let (mut term, _guard) = TerminalGuard::enter()?;
        let mut events = EventStream::new();
        run_wizard(&mut term, &mut events, w).await?
    };
    match outcome {
        Outcome::Finished(r) => println!("{}", summary(&r)),
        Outcome::Aborted => println!("setup aborted; nothing was written"),
    }
    Ok(())
}
