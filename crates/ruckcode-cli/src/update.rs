//! `rc self-update` and the once-a-day "update available" hint.
//!
//! Updates come from the public GitHub Pages site (the repository is private, so release assets
//! are not downloadable): `<base>/rc/latest.json` lists, per target triple, the archive `url`, its
//! `sha256` and a minisign `signature`. An archive is installed only if both the checksum and the
//! signature (verified against the public key embedded at build time from
//! `RUCKCODE_UPDATER_PUBKEY`) match; builds without a key refuse to update unless the user passes
//! `--insecure-skip-signature` (checksum only).

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::Args;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where updates are published (GitHub Pages).
pub const DEFAULT_BASE_URL: &str = "https://ruckc.github.io/ruckcode";
/// Overrides [`DEFAULT_BASE_URL`] (testing, mirrors).
pub const BASE_URL_ENV: &str = "RUCKCODE_UPDATE_BASE_URL";
/// `1` (or any non-empty value other than `0`) disables the daily hint.
pub const NO_CHECK_ENV: &str = "RUCKCODE_NO_UPDATE_CHECK";

/// Public key embedded at build time (empty when `RUCKCODE_UPDATER_PUBKEY` was not set).
const EMBEDDED_PUBKEY: &str = env!("RC_UPDATER_PUBKEY");
const TARGET: &str = env!("RC_TARGET");
const VERSION: &str = env!("RC_VERSION");

/// Archives larger than this are refused (the real ones are a few MB).
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const HINT_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Args, Debug, Clone, Default)]
pub struct SelfUpdateArgs {
    /// Only report whether a newer version is available; install nothing.
    #[arg(long)]
    pub check: bool,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Print only the latest published version (for scripts).
    #[arg(long)]
    pub version_only: bool,
    /// Install without verifying the minisign signature (checksum only). Dangerous: the
    /// checksum comes from the same server as the archive.
    #[arg(long)]
    pub insecure_skip_signature: bool,
}

// ---- manifest ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetEntry {
    pub url: String,
    pub sha256: String,
    /// Minisign signature: the contents of the `.sig` file (base64 of the minisig text, as
    /// written by `tauri signer sign`) or the plain minisig text.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub targets: BTreeMap<String, TargetEntry>,
}

pub fn parse_version(s: &str) -> Result<semver::Version> {
    let t = s.trim().trim_start_matches('v');
    semver::Version::parse(t).with_context(|| format!("invalid version `{s}`"))
}

/// True when `latest` is a strictly newer release than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => false,
    }
}

/// Development builds (`0.1.0-dev+abc123`) never nag.
pub fn is_dev_version(v: &str) -> bool {
    v.contains("-dev")
}

// ---- verification -----------------------------------------------------------------------

/// Accepts the Tauri form (base64 of the two-line minisign `.pub` file) or the bare `RW...` key.
pub fn parse_pubkey(s: &str) -> Result<minisign_verify::PublicKey> {
    let s = s.trim();
    if s.starts_with("RW") {
        return minisign_verify::PublicKey::from_base64(s)
            .map_err(|e| anyhow!("invalid public key: {e}"));
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s)
        .context("public key is neither a minisign key nor base64 of one")?;
    let text = String::from_utf8(raw).context("public key is not text")?;
    minisign_verify::PublicKey::decode(&text).map_err(|e| anyhow!("invalid public key: {e}"))
}

fn parse_signature(s: &str) -> Result<minisign_verify::Signature> {
    let s = s.trim();
    let text = if s.starts_with("untrusted comment") {
        s.to_string()
    } else {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(s.split_whitespace().collect::<String>())
            .context("signature is not valid base64")?;
        String::from_utf8(raw).context("signature is not text")?
    };
    minisign_verify::Signature::decode(&text).map_err(|e| anyhow!("invalid signature: {e}"))
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Check the archive against the manifest entry. `pubkey: None` skips the signature (the caller
/// has already decided that is acceptable). With a key, the signed trusted comment must also
/// carry `version:<version>` (`tauri signer sign --app-version`), so a tampered manifest cannot
/// pair a newer version number with an older, genuinely signed archive.
pub fn verify_archive(
    data: &[u8],
    entry: &TargetEntry,
    pubkey: Option<&str>,
    version: &str,
) -> Result<()> {
    let got = sha256_hex(data);
    let want = entry
        .sha256
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if want != got {
        bail!("checksum mismatch (expected {want}, got {got}); nothing was installed");
    }
    if let Some(pk) = pubkey {
        let pk = parse_pubkey(pk)?;
        let sig = parse_signature(&entry.signature)?;
        pk.verify(data, &sig, false)
            .map_err(|e| anyhow!("signature verification failed ({e}); nothing was installed"))?;
        let want = format!("version:{}", version.trim_start_matches('v'));
        if !sig.trusted_comment().split('\t').any(|f| f == want) {
            bail!(
                "signature is not bound to version {version} (signed: `{}`); nothing was installed",
                sig.trusted_comment()
            );
        }
    }
    Ok(())
}

// ---- archive and install ----------------------------------------------------------------

/// Pull the `rc` / `rc.exe` binary out of a release archive (`.tar.gz` or `.zip`).
pub fn extract_binary(archive: &[u8], url: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if url.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let name = f.name().map(|n| n.into_owned()).unwrap_or_default();
            if f.is_file() && file_name_of(&name) == "rc.exe" {
                f.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
    } else {
        let gz = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(gz);
        for entry in tar.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path()?.to_string_lossy().into_owned();
            if file_name_of(&path) == "rc" {
                entry.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
    }
    bail!("the archive does not contain the rc binary")
}

fn file_name_of(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// Replace `dest` with `bytes` atomically. With `running = true`, `dest` is the executable that
/// is running right now (handled by `self_replace`, which also works on Windows); otherwise it
/// is renamed over (used by tests and for other targets).
pub fn install_binary(bytes: &[u8], dest: &Path, running: bool) -> Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| anyhow!("no parent directory"))?;
    let tmp = dir.join(format!(".rc-update-{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("cannot write to {} (permission?)", dir.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        if running {
            self_replace::self_replace(&tmp)?;
        } else {
            std::fs::rename(&tmp, dest)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// `Some(hint)` when the binary was installed by a package manager that should update it.
pub fn managed_install(exe: &Path) -> Option<&'static str> {
    let p = exe.to_string_lossy().replace('\\', "/");
    if p.contains("/Cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
        Some("installed by Homebrew; run `brew upgrade rc` instead")
    } else if p.starts_with("/nix/store/") {
        Some("managed by Nix; update it through your Nix configuration")
    } else if p.starts_with("/snap/") {
        Some("installed as a snap; run `snap refresh` instead")
    } else if ["/usr/bin/", "/bin/", "/usr/sbin/", "/sbin/", "/usr/lib/"]
        .iter()
        .any(|d| p.starts_with(d))
    {
        Some("installed in a system directory by your package manager; update it with apt/dnf/pacman instead")
    } else {
        None
    }
}

// ---- network ----------------------------------------------------------------------------

pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

#[derive(Debug, Clone)]
pub struct Updater {
    pub base: String,
    /// Public key (Tauri base64 or bare minisign); `None` when not embedded.
    pub pubkey: Option<String>,
    pub current: String,
    pub target: String,
    client: reqwest::Client,
}

#[derive(Debug)]
pub enum Check {
    /// The site has no `rc/latest.json` (updates not configured for the release channel).
    NotPublished,
    UpToDate {
        latest: String,
    },
    Available {
        manifest: Box<Manifest>,
    },
}

impl Updater {
    pub fn from_build() -> Self {
        Self::new(
            base_url(),
            Some(EMBEDDED_PUBKEY.trim().to_string()).filter(|k| !k.is_empty()),
            VERSION.to_string(),
            TARGET.to_string(),
        )
    }

    pub fn new(base: String, pubkey: Option<String>, current: String, target: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("rc/{current}"))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self {
            base,
            pubkey,
            current,
            target,
            client,
        }
    }

    pub fn manifest_url(&self) -> String {
        format!("{}/rc/latest.json", self.base)
    }

    pub async fn check(&self) -> Result<Check> {
        let resp = self
            .client
            .get(self.manifest_url())
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .with_context(|| format!("cannot reach {}", self.manifest_url()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Check::NotPublished);
        }
        let resp = resp
            .error_for_status()
            .with_context(|| format!("fetching {}", self.manifest_url()))?;
        let manifest: Manifest = resp
            .json()
            .await
            .context("the update manifest is not valid JSON")?;
        parse_version(&manifest.version)?;
        if is_newer(&manifest.version, &self.current) {
            Ok(Check::Available {
                manifest: Box::new(manifest),
            })
        } else {
            Ok(Check::UpToDate {
                latest: manifest.version,
            })
        }
    }

    /// Download the archive for this target and verify it. `insecure` skips the signature.
    pub async fn download(&self, manifest: &Manifest, insecure: bool) -> Result<Vec<u8>> {
        let entry = manifest.targets.get(&self.target).ok_or_else(|| {
            anyhow!(
                "release {} has no build for {} (available: {})",
                manifest.version,
                self.target,
                manifest
                    .targets
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let url = if entry.url.starts_with("http://") || entry.url.starts_with("https://") {
            entry.url.clone()
        } else {
            format!("{}/{}", self.base, entry.url.trim_start_matches('/'))
        };
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .with_context(|| format!("downloading {url}"))?;
        if resp.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
            bail!("refusing to download {url}: unexpectedly large");
        }
        let data = resp.bytes().await.context("download interrupted")?;
        if data.len() as u64 > MAX_DOWNLOAD {
            bail!("refusing {url}: unexpectedly large");
        }
        let pubkey = if insecure {
            None
        } else {
            Some(self.pubkey.as_deref().ok_or_else(not_configured)?)
        };
        verify_archive(&data, entry, pubkey, &manifest.version)?;
        extract_binary(&data, &url)
    }
}

fn not_configured() -> anyhow::Error {
    anyhow!(
        "this build of rc has no update signing key embedded (a development or unconfigured \
         build), so updates cannot be verified. Install a release build from {DEFAULT_BASE_URL}, \
         or pass --insecure-skip-signature to accept a checksum-only update"
    )
}

// ---- rc self-update ---------------------------------------------------------------------

pub async fn run(args: &SelfUpdateArgs) -> Result<()> {
    let updater = Updater::from_build();
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("cannot locate the running binary")?;
    run_with(&updater, args, &exe, true, &mut std::io::stdout()).await
}

/// The testable body of `rc self-update`. `running` says whether `exe` is the live process
/// binary (see [`install_binary`]).
pub async fn run_with(
    updater: &Updater,
    args: &SelfUpdateArgs,
    exe: &Path,
    running: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let check = updater.check().await?;
    let manifest = match check {
        Check::NotPublished => {
            if args.version_only {
                bail!(
                    "no update manifest is published at {}",
                    updater.manifest_url()
                );
            }
            writeln!(
                out,
                "Updates are not configured for this release channel (no manifest at {}).",
                updater.manifest_url()
            )?;
            return Ok(());
        }
        Check::UpToDate { latest } => {
            if args.version_only {
                writeln!(out, "{latest}")?;
            } else {
                writeln!(
                    out,
                    "rc {} is up to date (latest: {latest}).",
                    updater.current
                )?;
            }
            return Ok(());
        }
        Check::Available { manifest } => *manifest,
    };
    if args.version_only {
        writeln!(out, "{}", manifest.version)?;
        return Ok(());
    }
    writeln!(
        out,
        "rc {} is available (you have {}).",
        manifest.version, updater.current
    )?;
    let notes = manifest.notes.trim();
    if !notes.is_empty() {
        for line in notes.lines().take(20) {
            writeln!(out, "  {line}")?;
        }
    }
    if args.check {
        writeln!(out, "Run `rc self-update` to install it.")?;
        return Ok(());
    }
    if updater.pubkey.is_none() && !args.insecure_skip_signature {
        return Err(not_configured());
    }
    if let Some(why) = managed_install(exe) {
        bail!(
            "{} is {why}. Self-update only replaces binaries you installed yourself",
            exe.display()
        );
    }
    if args.insecure_skip_signature {
        eprintln!(
            "WARNING: --insecure-skip-signature: the minisign signature will NOT be verified. \
             Only the SHA-256 from the same server is checked, which does not protect against a \
             compromised server."
        );
    }
    if !args.yes {
        if !std::io::stdin().is_terminal() {
            bail!("not a terminal; pass --yes to install without confirmation");
        }
        write!(
            out,
            "Install rc {} over {}? [y/N] ",
            manifest.version,
            exe.display()
        )?;
        out.flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            writeln!(out, "Cancelled.")?;
            return Ok(());
        }
    }
    writeln!(out, "Downloading and verifying...")?;
    let bytes = updater
        .download(&manifest, args.insecure_skip_signature)
        .await?;
    install_binary(&bytes, exe, running)?;
    writeln!(out, "Updated to rc {}.", manifest.version)?;
    Ok(())
}

// ---- daily hint -------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cache {
    /// Unix seconds of the last check.
    pub checked_at: u64,
    /// Latest published version seen then.
    pub latest: Option<String>,
}

pub fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("ruckcode").join("update-check.json"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn read_cache(path: &Path) -> Cache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_cache(path: &Path, cache: &Cache) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, text);
    }
}

pub fn hint_text(latest: &str) -> String {
    format!("rc {latest} is available (you have {VERSION}): run `rc self-update`")
}

fn env_disables(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty() && v != "0")
}

/// What the interactive entry points should do about update hints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HintPlan {
    /// A newer version to announce right away, from the cached result of an earlier check.
    pub cached: Option<String>,
    /// The cache is older than a day: refresh it in the background ([`refresh_cache`]).
    pub refresh: bool,
}

/// Pure decision core of the hint, so it can be tested without a clock or a filesystem.
pub fn plan_hint(
    enabled: bool,
    cache: &Cache,
    now: u64,
    current: &str,
    have_pubkey: bool,
) -> HintPlan {
    if !enabled || !have_pubkey || is_dev_version(current) {
        return HintPlan::default();
    }
    let cached = cache
        .latest
        .as_deref()
        .filter(|l| is_newer(l, current))
        .map(str::to_string);
    HintPlan {
        cached,
        refresh: now.saturating_sub(cache.checked_at) >= HINT_INTERVAL_SECS,
    }
}

/// Plan for this process: honours `RUCKCODE_NO_UPDATE_CHECK`, the `check_updates` config option,
/// dev builds and unconfigured (no embedded key) builds.
pub fn startup_plan(config_enabled: bool) -> HintPlan {
    let enabled = config_enabled && !env_disables(std::env::var(NO_CHECK_ENV).ok().as_deref());
    let Some(path) = cache_path() else {
        return HintPlan::default();
    };
    plan_hint(
        enabled,
        &read_cache(&path),
        now_secs(),
        VERSION,
        !EMBEDDED_PUBKEY.trim().is_empty(),
    )
}

/// Query the site (short timeout) and store the result; returns the version when newer. Failures
/// are swallowed (but still stamp the cache so an offline machine retries only daily).
pub async fn refresh_cache() -> Option<String> {
    let path = cache_path()?;
    refresh_cache_with(&Updater::from_build(), &path).await
}

pub async fn refresh_cache_with(updater: &Updater, path: &Path) -> Option<String> {
    let checked = tokio::time::timeout(Duration::from_secs(5), updater.check()).await;
    let latest = match checked {
        Ok(Ok(Check::Available { manifest })) => Some(manifest.version),
        Ok(Ok(Check::UpToDate { latest })) => Some(latest),
        _ => None,
    };
    write_cache(
        path,
        &Cache {
            checked_at: now_secs(),
            latest: latest.clone(),
        },
    );
    latest.filter(|l| is_newer(l, &updater.current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    // ---- throwaway key pair and signing --------------------------------------------------

    struct Keys {
        /// Tauri-style public key: base64 of the minisign `.pub` file.
        pubkey_b64: String,
        sk: minisign::SecretKey,
    }

    fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pub_text = kp.pk.to_box().unwrap().to_string();
        Keys {
            pubkey_b64: base64::engine::general_purpose::STANDARD.encode(pub_text),
            sk: kp.sk,
        }
    }

    /// A `.sig` the way `tauri signer sign` writes it: base64 of the minisig text.
    fn sign(k: &Keys, data: &[u8], version: &str) -> String {
        let comment = format!("timestamp:1\tfile:rc.tar.gz\tversion:{version}");
        let sig = minisign::sign(
            None,
            &k.sk,
            std::io::Cursor::new(data),
            Some(&comment),
            None,
        )
        .unwrap()
        .to_string();
        base64::engine::general_purpose::STANDARD.encode(sig)
    }

    fn tar_gz(name: &str, content: &[u8]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(content.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, name, content).unwrap();
        b.into_inner().unwrap().finish().unwrap()
    }

    // ---- local HTTP server ---------------------------------------------------------------

    /// Serves fixed paths from memory on 127.0.0.1; 404 for anything else.
    struct Server {
        base: String,
        hits: Arc<Mutex<Vec<String>>>,
    }

    fn serve(files: Vec<(String, Vec<u8>)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let h2 = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("/")
                    .to_string();
                h2.lock().unwrap().push(path.clone());
                let (status, body) = match files.iter().find(|(p, _)| *p == path) {
                    Some((_, b)) => ("200 OK", b.clone()),
                    None => ("404 Not Found", b"missing".to_vec()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        Server { base, hits }
    }

    const TGT: &str = "x86_64-unknown-linux-musl";

    struct Fixture {
        server: Server,
        keys: Keys,
    }

    /// A release `version` whose archive holds a binary with `payload`; `tweak` may corrupt the
    /// manifest entry or archive before serving.
    fn fixture(
        version: &str,
        payload: &[u8],
        tweak: impl Fn(&Keys, &mut TargetEntry, &mut Vec<u8>),
    ) -> Fixture {
        let keys = keys();
        let mut archive = tar_gz(&format!("rc-{version}-{TGT}/rc"), payload);
        let mut entry = TargetEntry {
            url: format!("download/v{version}/rc-{version}-{TGT}.tar.gz"),
            sha256: sha256_hex(&archive),
            signature: sign(&keys, &archive, version),
        };
        tweak(&keys, &mut entry, &mut archive);
        let manifest = Manifest {
            version: version.to_string(),
            notes: "- fixed things".into(),
            pub_date: "2026-01-01T00:00:00Z".into(),
            targets: BTreeMap::from([(TGT.to_string(), entry.clone())]),
        };
        let server = serve(vec![
            (
                "/rc/latest.json".into(),
                serde_json::to_vec(&manifest).unwrap(),
            ),
            (format!("/{}", entry.url), archive),
        ]);
        Fixture { server, keys }
    }

    fn updater(f: &Fixture, current: &str, pubkey: bool) -> Updater {
        Updater::new(
            f.server.base.clone(),
            pubkey.then(|| f.keys.pubkey_b64.clone()),
            current.into(),
            TGT.into(),
        )
    }

    fn args() -> SelfUpdateArgs {
        SelfUpdateArgs {
            yes: true,
            ..Default::default()
        }
    }

    async fn run_in(up: &Updater, a: &SelfUpdateArgs, exe: &Path) -> (Result<()>, String) {
        let mut out = Vec::new();
        let r = run_with(up, a, exe, false, &mut out).await;
        (r, String::from_utf8(out).unwrap())
    }

    fn temp_exe(dir: &tempfile::TempDir) -> PathBuf {
        let p = dir.path().join("rc");
        std::fs::write(&p, b"old").unwrap();
        p
    }

    // ---- tests ---------------------------------------------------------------------------

    #[test]
    fn version_comparison() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        // A dev build of 0.1.0 is older than the 0.1.0 release.
        assert!(is_newer("0.1.0", "0.1.0-dev+abc123"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(is_dev_version("0.1.0-dev+abc"));
        assert!(!is_dev_version("0.1.0"));
    }

    #[tokio::test]
    async fn up_to_date() {
        let f = fixture("0.2.0", b"new", |_, _, _| {});
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, out) = run_in(&updater(&f, "0.2.0", true), &args(), &exe).await;
        r.unwrap();
        assert!(out.contains("up to date"), "{out}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        // Only the manifest was fetched.
        assert_eq!(*f.server.hits.lock().unwrap(), ["/rc/latest.json"]);
    }

    #[tokio::test]
    async fn manifest_missing_means_not_configured() {
        let server = serve(vec![]);
        let up = Updater::new(server.base.clone(), None, "0.1.0".into(), TGT.into());
        let dir = tempfile::tempdir().unwrap();
        let (r, out) = run_in(&up, &args(), &temp_exe(&dir)).await;
        r.unwrap();
        assert!(out.contains("not configured"), "{out}");
    }

    #[tokio::test]
    async fn check_and_version_only_install_nothing() {
        let f = fixture("0.2.0", b"new", |_, _, _| {});
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let up = updater(&f, "0.1.0", true);
        let a = SelfUpdateArgs {
            check: true,
            ..Default::default()
        };
        let (r, out) = run_in(&up, &a, &exe).await;
        r.unwrap();
        assert!(
            out.contains("0.2.0 is available") && out.contains("fixed things"),
            "{out}"
        );
        let a = SelfUpdateArgs {
            version_only: true,
            ..Default::default()
        };
        let (r, out) = run_in(&up, &a, &exe).await;
        r.unwrap();
        assert_eq!(out, "0.2.0\n");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert_eq!(
            f.server.hits.lock().unwrap().len(),
            2,
            "no archive download"
        );
    }

    #[tokio::test]
    async fn newer_version_is_installed() {
        let f = fixture("0.2.0", b"NEW-BINARY", |_, _, _| {});
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, out) = run_in(&updater(&f, "0.1.0", true), &args(), &exe).await;
        r.unwrap();
        assert!(out.contains("Updated to rc 0.2.0"), "{out}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW-BINARY");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn bad_sha_is_rejected() {
        let f = fixture("0.2.0", b"NEW", |_, e, _| e.sha256 = "0".repeat(64));
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, _) = run_in(&updater(&f, "0.1.0", true), &args(), &exe).await;
        let e = r.unwrap_err().to_string();
        assert!(e.contains("checksum mismatch"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn bad_signature_is_rejected() {
        // Signed with a different key than the one the updater trusts, and checksum is right.
        let f = fixture("0.2.0", b"NEW", |_, e, archive| {
            e.signature = sign(&keys(), archive, "0.2.0");
        });
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, _) = run_in(&updater(&f, "0.1.0", true), &args(), &exe).await;
        let e = r.unwrap_err().to_string();
        assert!(e.contains("signature verification failed"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn signature_bound_to_another_version_is_rejected() {
        // A genuine signature of an older release, replayed under a newer manifest version.
        let f = fixture("0.2.0", b"NEW", |k, e, archive| {
            e.signature = sign(k, archive, "0.0.9");
        });
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, _) = run_in(&updater(&f, "0.1.0", true), &args(), &exe).await;
        let e = r.unwrap_err().to_string();
        assert!(e.contains("not bound to version 0.2.0"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn tampered_archive_with_matching_manifest_checksum_fails_signature() {
        // An attacker who controls the server can fix the checksum but cannot forge the signature.
        let f = fixture("0.2.0", b"NEW", |_, e, archive| {
            *archive = tar_gz("rc", b"EVIL");
            e.sha256 = sha256_hex(archive);
        });
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let (r, _) = run_in(&updater(&f, "0.1.0", true), &args(), &exe).await;
        assert!(r
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn missing_pubkey_refuses_unless_insecure() {
        let f = fixture("0.2.0", b"NEW", |_, _, _| {});
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let up = updater(&f, "0.1.0", false);
        let (r, _) = run_in(&up, &args(), &exe).await;
        let e = r.unwrap_err().to_string();
        assert!(e.contains("no update signing key"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        // --check still works without a key.
        let a = SelfUpdateArgs {
            check: true,
            ..Default::default()
        };
        run_in(&up, &a, &exe).await.0.unwrap();
        // The explicit opt-out installs (checksum only).
        let a = SelfUpdateArgs {
            insecure_skip_signature: true,
            ..args()
        };
        run_in(&up, &a, &exe).await.0.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW");
    }

    #[tokio::test]
    async fn insecure_still_checks_sha() {
        let f = fixture("0.2.0", b"NEW", |_, e, _| e.sha256 = "ab".repeat(32));
        let dir = tempfile::tempdir().unwrap();
        let exe = temp_exe(&dir);
        let a = SelfUpdateArgs {
            insecure_skip_signature: true,
            ..args()
        };
        let (r, _) = run_in(&updater(&f, "0.1.0", false), &a, &exe).await;
        assert!(r.unwrap_err().to_string().contains("checksum mismatch"));
    }

    #[tokio::test]
    async fn unsupported_target_is_reported() {
        let f = fixture("0.2.0", b"NEW", |_, _, _| {});
        let mut up = updater(&f, "0.1.0", true);
        up.target = "riscv64-unknown-linux-gnu".into();
        let dir = tempfile::tempdir().unwrap();
        let (r, _) = run_in(&up, &args(), &temp_exe(&dir)).await;
        assert!(r
            .unwrap_err()
            .to_string()
            .contains("has no build for riscv64"));
    }

    #[tokio::test]
    async fn package_manager_installs_are_refused() {
        let f = fixture("0.2.0", b"NEW", |_, _, _| {});
        let up = updater(&f, "0.1.0", true);
        let (r, _) = run_in(
            &up,
            &args(),
            Path::new("/opt/homebrew/Cellar/rc/0.1.0/bin/rc"),
        )
        .await;
        assert!(r.unwrap_err().to_string().contains("Homebrew"));
        let (r, _) = run_in(&up, &args(), Path::new("/usr/bin/rc")).await;
        assert!(r.unwrap_err().to_string().contains("package manager"));
    }

    #[test]
    fn managed_install_detection() {
        for p in [
            "/usr/bin/rc",
            "/opt/homebrew/Cellar/rc/1.0/bin/rc",
            "/usr/local/Cellar/rc/1.0/bin/rc",
            "/home/linuxbrew/.linuxbrew/Cellar/rc/1.0/bin/rc",
            "/nix/store/abc-rc/bin/rc",
        ] {
            assert!(managed_install(Path::new(p)).is_some(), "{p}");
        }
        for p in [
            "/home/u/.local/bin/rc",
            "/usr/local/bin/rc",
            "C:\\Users\\u\\bin\\rc.exe",
        ] {
            assert!(managed_install(Path::new(p)).is_none(), "{p}");
        }
    }

    #[test]
    fn pubkey_forms_parse() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let text = kp.pk.to_box().unwrap().to_string();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&text);
        parse_pubkey(&b64).unwrap();
        parse_pubkey(&kp.pk.to_base64()).unwrap();
        assert!(parse_pubkey("not a key").is_err());
    }

    #[test]
    fn extract_tar_and_zip() {
        let t = tar_gz("rc-1.0.0-x/rc", b"bin");
        assert_eq!(extract_binary(&t, "a.tar.gz").unwrap(), b"bin");
        assert!(extract_binary(&tar_gz("README.md", b"x"), "a.tar.gz").is_err());
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file(
                "rc-1.0.0-x/rc.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(b"winbin").unwrap();
            z.finish().unwrap();
        }
        assert_eq!(
            extract_binary(&buf.into_inner(), "a.zip").unwrap(),
            b"winbin"
        );
    }

    #[test]
    fn install_binary_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("rc");
        std::fs::write(&dest, b"old").unwrap();
        install_binary(b"new", &dest, false).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn hint_plan_rules() {
        let day = HINT_INTERVAL_SECS;
        let fresh = Cache {
            checked_at: 1_000,
            latest: Some("0.2.0".into()),
        };
        // Disabled by config/env, dev build, or no embedded key: silent, no refresh.
        for (en, key, cur) in [
            (false, true, "0.1.0"),
            (true, false, "0.1.0"),
            (true, true, "0.1.0-dev+abc"),
        ] {
            assert_eq!(plan_hint(en, &fresh, 1_000, cur, key), HintPlan::default());
        }
        // Checked recently: cached hint, no refresh.
        let p = plan_hint(true, &fresh, 1_000 + day - 1, "0.1.0", true);
        assert!(p.cached.as_deref() == Some("0.2.0") && !p.refresh);
        // Stale cache: still shows the old result and refreshes.
        let p = plan_hint(true, &fresh, 1_000 + day, "0.1.0", true);
        assert!(p.cached.is_some() && p.refresh);
        // Already current: nothing to show.
        let p = plan_hint(true, &fresh, 1_000, "0.2.0", true);
        assert!(p.cached.is_none());
        // Empty cache: refresh.
        assert!(plan_hint(true, &Cache::default(), 5, "0.1.0", true).refresh || day > 5);
        assert!(plan_hint(true, &Cache::default(), day + 5, "0.1.0", true).refresh);
    }

    #[test]
    fn env_switch() {
        assert!(env_disables(Some("1")));
        assert!(env_disables(Some("true")));
        assert!(!env_disables(Some("0")));
        assert!(!env_disables(Some("")));
        assert!(!env_disables(None));
    }

    #[tokio::test]
    async fn refresh_writes_cache_and_survives_failures() {
        let f = fixture("0.2.0", b"NEW", |_, _, _| {});
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/update-check.json");
        let hint = refresh_cache_with(&updater(&f, "0.1.0", true), &path).await;
        assert_eq!(hint.as_deref(), Some("0.2.0"));
        let c = read_cache(&path);
        assert_eq!(c.latest.as_deref(), Some("0.2.0"));
        assert!(c.checked_at > 0);
        // Unreachable server: no hint, cache stamped so we do not retry until tomorrow.
        let down = Updater::new(
            "http://127.0.0.1:9".into(),
            None,
            "0.1.0".into(),
            TGT.into(),
        );
        let path2 = dir.path().join("two.json");
        assert!(refresh_cache_with(&down, &path2).await.is_none());
        assert!(read_cache(&path2).checked_at > 0);
    }
}
