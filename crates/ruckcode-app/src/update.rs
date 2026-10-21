//! In-app updates: check the public GitHub Pages site, download in the background, install on
//! restart.
//!
//! The repository is private, so releases are not downloadable from GitHub; the release workflow
//! publishes the latest version to Pages (`<base>/latest.json` plus the signed bundles). We wrap
//! `tauri-plugin-updater` in our own commands instead of exposing the plugin to the webview: the
//! webview gets four narrow commands, the base URL and the "not configured" and "manual update"
//! cases are decided here, and the downloaded bytes never leave Rust.
//!
//! Flow driven by the UI: `update_check` stages the update, `update_download` fetches it with
//! progress events, `update_install` applies it (on Windows this runs the MSI installer and
//! the process exits), `update_restart` relaunches.

use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

/// Where releases are published (GitHub Pages of the private repository).
pub const DEFAULT_BASE_URL: &str = "https://ruckc.github.io/ruckcode";
/// Overrides [`DEFAULT_BASE_URL`] (testing against a staging site). Must be https.
pub const BASE_URL_ENV: &str = "RUCKCODE_UPDATE_BASE_URL";

pub fn base_url_from(env: Option<&str>) -> String {
    env.map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

pub fn base_url() -> String {
    base_url_from(std::env::var(BASE_URL_ENV).ok().as_deref())
}

pub fn manifest_url(base: &str) -> String {
    format!("{base}/latest.json")
}

/// The updater public key (Tauri format: base64 of the minisign `.pub` file) baked in at build
/// time from `RUCKCODE_UPDATER_PUBKEY`, if any.
fn compiled_pubkey() -> Option<&'static str> {
    option_env!("RUCKCODE_UPDATER_PUBKEY")
        .map(str::trim)
        .filter(|k| !k.is_empty())
}

/// Public key from the build-time env var, else the `plugins.updater.pubkey` of the merged
/// `tauri.conf.json` (CI merges it with `tauri build --config`). `None` means "not configured".
pub fn pubkey(app: &AppHandle) -> Option<String> {
    compiled_pubkey().map(str::to_string).or_else(|| {
        app.config()
            .plugins
            .0
            .get("updater")
            .and_then(|u| u.get("pubkey"))
            .and_then(|k| k.as_str())
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
    })
}

/// Why this installation cannot update itself, or `None` when it can. The updater swaps the app
/// in place, which works for AppImage, MSI and macOS app bundles (the manifest has a
/// per-bundle key for each; a legacy NSIS install, no longer shipped, still counts as updatable
/// and receives the MSI through the generic key); a `.deb`/`.rpm` install
/// (or an unbundled dev build) must be updated by hand from the download page.
pub fn manual_reason(bundle: Option<&BundleType>) -> Option<&'static str> {
    match bundle {
        Some(
            BundleType::AppImage
            | BundleType::Nsis
            | BundleType::Msi
            | BundleType::App
            | BundleType::Dmg,
        ) => None,
        Some(BundleType::Deb | BundleType::Rpm) => Some(
            "This copy was installed from a Linux package, which cannot update itself. \
             Download the new version from the download page (the AppImage updates itself).",
        ),
        _ => Some("This is not an installed build, so it cannot update itself."),
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CheckResult {
    /// No signing key in this build, or no manifest published: updates are off.
    NotConfigured {
        reason: String,
    },
    UpToDate {
        version: String,
    },
    Available {
        version: String,
        current: String,
        notes: Option<String>,
        date: Option<String>,
        /// Set when the update cannot be installed automatically: why, and where to get it.
        manual_reason: Option<String>,
        download_page: String,
    },
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

struct Staged {
    update: Update,
    bytes: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct UpdateState {
    staged: Mutex<Option<Staged>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<CheckResult, String> {
    let Some(key) = pubkey(&app) else {
        return Ok(CheckResult::NotConfigured {
            reason: "This build has no update signing key, so it cannot verify updates.".into(),
        });
    };
    let base = base_url();
    let endpoint = manifest_url(&base).parse::<tauri::Url>().map_err(err)?;
    let updater = app
        .updater_builder()
        .pubkey(key)
        .endpoints(vec![endpoint])
        .map_err(err)?
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(err)?;
    match updater.check().await {
        Ok(Some(update)) => {
            let manual = manual_reason(tauri::utils::platform::bundle_type().as_ref());
            let result = CheckResult::Available {
                version: update.version.clone(),
                current: update.current_version.clone(),
                notes: update.body.clone().filter(|b| !b.trim().is_empty()),
                date: update.date.map(|d| d.to_string()),
                manual_reason: manual.map(str::to_string),
                download_page: format!("{base}/"),
            };
            *state.staged.lock().await = manual.is_none().then_some(Staged {
                update,
                bytes: None,
            });
            Ok(result)
        }
        Ok(None) => {
            *state.staged.lock().await = None;
            Ok(CheckResult::UpToDate {
                version: app.package_info().version.to_string(),
            })
        }
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => Ok(CheckResult::NotConfigured {
            reason: format!(
                "No update manifest is published at {}.",
                manifest_url(&base)
            ),
        }),
        Err(e) => Err(err(e)),
    }
}

/// Download the staged update (signature verified by the updater); progress goes to `on_event`.
#[tauri::command]
pub async fn update_download(
    state: State<'_, UpdateState>,
    on_event: Channel<DownloadEvent>,
) -> Result<(), String> {
    let update = {
        let guard = state.staged.lock().await;
        let staged = guard.as_ref().ok_or("no update is pending; check first")?;
        staged.update.clone()
    };
    let mut first = true;
    let bytes = update
        .download(
            |chunk, total| {
                if std::mem::take(&mut first) {
                    let _ = on_event.send(DownloadEvent::Started {
                        content_length: total,
                    });
                }
                let _ = on_event.send(DownloadEvent::Progress {
                    chunk_length: chunk,
                });
            },
            || {
                let _ = on_event.send(DownloadEvent::Finished);
            },
        )
        .await
        .map_err(err)?;
    if let Some(s) = state.staged.lock().await.as_mut() {
        s.bytes = Some(bytes);
    }
    Ok(())
}

/// Apply the downloaded update. On Windows the installer runs and this process exits.
#[tauri::command]
pub async fn update_install(state: State<'_, UpdateState>) -> Result<(), String> {
    let guard = state.staged.lock().await;
    let staged = guard.as_ref().ok_or("no update is pending")?;
    let bytes = staged
        .bytes
        .as_ref()
        .ok_or("the update is not downloaded yet")?;
    staged.update.install(bytes).map_err(err)
}

/// Relaunch the app (after `update_install`).
#[tauri::command]
pub fn update_restart(app: AppHandle) -> Result<(), String> {
    app.restart()
}

/// Register the updater plugin. Its own commands stay unreachable from the webview: the
/// capability grants only the `update_*` commands above.
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    let mut b = tauri_plugin_updater::Builder::new();
    if let Some(key) = compiled_pubkey() {
        b = b.pubkey(key);
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_default_and_override() {
        assert_eq!(base_url_from(None), DEFAULT_BASE_URL);
        assert_eq!(base_url_from(Some("")), DEFAULT_BASE_URL);
        assert_eq!(base_url_from(Some("  ")), DEFAULT_BASE_URL);
        assert_eq!(
            base_url_from(Some("https://staging.example/x/")),
            "https://staging.example/x"
        );
        assert_eq!(
            manifest_url("https://staging.example/x"),
            "https://staging.example/x/latest.json"
        );
        assert_eq!(
            manifest_url(DEFAULT_BASE_URL),
            "https://ruckc.github.io/ruckcode/latest.json"
        );
    }

    #[test]
    fn only_in_place_bundles_update_themselves() {
        for ok in [
            BundleType::AppImage,
            BundleType::Nsis,
            BundleType::Msi,
            BundleType::App,
            BundleType::Dmg,
        ] {
            assert!(manual_reason(Some(&ok)).is_none(), "{ok:?}");
        }
        assert!(manual_reason(Some(&BundleType::Deb))
            .unwrap()
            .contains("AppImage"));
        assert!(manual_reason(Some(&BundleType::Rpm)).is_some());
        assert!(manual_reason(None)
            .unwrap()
            .contains("not an installed build"));
    }

    /// The committed config must deserialize into the plugin's own config type (a failure here
    /// would abort the app at startup), point at the same site as the code, and not ship a key.
    #[test]
    fn committed_updater_config_is_valid() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let cfg: tauri_plugin_updater::Config =
            serde_json::from_value(conf["plugins"]["updater"].clone()).unwrap();
        assert_eq!(
            cfg.endpoints.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
            [manifest_url(DEFAULT_BASE_URL)]
        );
        assert!(cfg.require_signed_version, "rollback protection stays on");
        assert!(
            cfg.pubkey.is_empty(),
            "the real key is merged in by CI, never committed"
        );
        assert!(!cfg.dangerous_insecure_transport_protocol && !cfg.dangerous_accept_invalid_certs);
        // And a merged config (what CI passes with --config) parses too.
        let merged = serde_json::json!({
            "endpoints": ["https://x.example/latest.json"], "pubkey": "QUJD",
            "requireSignedVersion": true
        });
        assert!(serde_json::from_value::<tauri_plugin_updater::Config>(merged).is_ok());
    }

    #[test]
    fn check_result_wire_format() {
        let v = serde_json::to_value(CheckResult::Available {
            version: "1.2.3".into(),
            current: "1.0.0".into(),
            notes: Some("n".into()),
            date: None,
            manual_reason: None,
            download_page: "https://x/".into(),
        })
        .unwrap();
        assert_eq!(v["state"], "available");
        assert_eq!(v["downloadPage"], "https://x/");
        assert!(v["manualReason"].is_null());
        let v = serde_json::to_value(CheckResult::NotConfigured { reason: "r".into() }).unwrap();
        assert_eq!(
            (v["state"].as_str(), v["reason"].as_str()),
            (Some("notConfigured"), Some("r"))
        );
        let v = serde_json::to_value(CheckResult::UpToDate {
            version: "1".into(),
        })
        .unwrap();
        assert_eq!(v["state"], "upToDate");
        let e = serde_json::to_value(DownloadEvent::Progress { chunk_length: 5 }).unwrap();
        assert_eq!(e["event"], "progress");
        assert_eq!(e["data"]["chunkLength"], 5);
        assert_eq!(
            serde_json::to_value(DownloadEvent::Finished).unwrap()["event"],
            "finished"
        );
    }
}
