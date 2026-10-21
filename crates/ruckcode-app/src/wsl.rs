//! WSL2 support: distro discovery, `rc` provisioning inside a distro and path translation.
//!
//! Everything that parses or builds text is a pure function so it is testable on any OS; only the
//! thin `run_wsl*` helpers actually launch `wsl.exe`, and those refuse to run off Windows.

use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

/// GitHub `owner/repo` the release archives are downloaded from.
pub const RELEASE_REPO: &str = "ruckc/ruckcode";

/// `~/.local/bin` first, so a freshly installed `rc` is found even by login shells that did not
/// add it to `PATH` (Debian/Ubuntu only do so when the directory existed at login).
pub const PATH_PREFIX: &str = "PATH=$HOME/.local/bin:$PATH";

// ---- decoding and parsing -----------------------------------------------------------------

/// Decode output of `wsl.exe` itself. Its own messages and listings are UTF-16LE (usually with a
/// BOM); output of commands run *inside* a distro is UTF-8. Detect which, strip BOM, NULs and CRs.
pub fn decode_wsl_output(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || (bytes.len() >= 4
            && bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count() * 2 > bytes.len() / 2);
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{feff}')
        .replace(['\r', '\0'], "")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Distro {
    pub name: String,
    /// "Running", "Stopped", ... (localized by Windows; `None` from the `-l -q` fallback).
    pub state: Option<String>,
    pub version: Option<u8>,
    pub default: bool,
    /// Only WSL2 distros that are real user distros can host `rc`.
    pub selectable: bool,
    /// Why it cannot be selected.
    pub reason: Option<String>,
}

fn distro(name: &str, state: Option<String>, version: Option<u8>, default: bool) -> Distro {
    let reason = if name.starts_with("docker-desktop") {
        Some("Docker Desktop internal distro".to_string())
    } else if version == Some(1) {
        Some("WSL1 distro (WSL2 required)".to_string())
    } else {
        None
    };
    Distro {
        name: name.to_string(),
        state,
        version,
        default,
        selectable: reason.is_none(),
        reason,
    }
}

/// Parse `wsl.exe -l -v`. The header is localized, so rows are recognised structurally: an
/// optional `*` default marker, the name, the (possibly multi-word) state and a numeric version.
pub fn parse_distro_list(text: &str) -> Vec<Distro> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (default, rest) = match line.strip_prefix('*') {
            Some(r) => (true, r.trim_start()),
            None => (false, line),
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.len() < 3 {
            continue;
        }
        let Ok(version) = tokens[tokens.len() - 1].parse::<u8>() else {
            continue; // header or a message line
        };
        let state = tokens[1..tokens.len() - 1].join(" ");
        out.push(distro(tokens[0], Some(state), Some(version), default));
    }
    out
}

/// Parse `wsl.exe -l -q` (names only; versions unknown, so assumed selectable).
pub fn parse_distro_names(text: &str) -> Vec<Distro> {
    text.lines()
        .map(|l| l.trim().trim_start_matches('*').trim())
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .map(|n| distro(n, None, None, false))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RcStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// The version this app expects (its own).
    pub expected: String,
}

/// Parse the output of `command -v rc && rc --version`.
pub fn parse_rc_check(output: &str, expected: &str) -> RcStatus {
    let mut path = None;
    let mut version = None;
    for line in output.lines().map(str::trim) {
        if path.is_none() && line.starts_with('/') {
            path = Some(line.to_string());
        } else if version.is_none() {
            version = line
                .split_whitespace()
                .find(|t| looks_like_version(t))
                .map(|t| t.trim_start_matches('v').to_string());
        }
    }
    RcStatus {
        installed: path.is_some(),
        path,
        version,
        expected: expected.to_string(),
    }
}

fn looks_like_version(tok: &str) -> bool {
    let t = tok.trim_start_matches('v');
    let mut parts = t.splitn(3, '.');
    let digits = |s: Option<&str>| {
        s.is_some_and(|s| !s.is_empty() && s.chars().next().unwrap().is_ascii_digit())
    };
    digits(parts.next()) && digits(parts.next()) && digits(parts.next())
}

// ---- path translation ---------------------------------------------------------------------

fn collapse(path: &str) -> String {
    let mut out = String::from("/");
    let mut first = true;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if !first {
            out.push('/');
        }
        out.push_str(seg);
        first = false;
    }
    out
}

/// Translate a Windows or Linux path to the Linux path to use inside `distro`.
///
/// * `\\wsl$\<distro>\home\x` and `\\wsl.localhost\<distro>\...` become `/home/x`; a share of a
///   different distro is rejected.
/// * `C:\Users\x` becomes `/mnt/c/Users/x` (drive letters are case-insensitive).
/// * Absolute Linux paths and `~` paths pass through.
pub fn windows_to_linux(path: &str, distro: &str) -> Result<String, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("path is empty".into());
    }
    let p = p
        .strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .unwrap_or_else(|| p.strip_prefix(r"\\?\").unwrap_or(p).to_string());
    let norm = p.replace('\\', "/");

    if let Some(rest) = norm.strip_prefix("//") {
        let mut it = rest.splitn(3, '/');
        let host = it.next().unwrap_or("").to_ascii_lowercase();
        let share = it.next().unwrap_or("");
        let tail = it.next().unwrap_or("");
        if host == "wsl$" || host == "wsl.localhost" {
            if share.is_empty() {
                return Err("the WSL share path must name a distro".into());
            }
            if !share.eq_ignore_ascii_case(distro) {
                return Err(format!(
                    "path belongs to distro `{share}`, but `{distro}` is selected"
                ));
            }
            return Ok(collapse(tail));
        }
        return Err(format!(
            "network path `{path}` is not reachable from WSL; use a drive letter or a path inside the distro"
        ));
    }
    let bytes = norm.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if bytes.len() > 2 && bytes[2] != b'/' {
            return Err(format!("drive-relative path `{path}` is not supported"));
        }
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let tail = collapse(&norm[2..]);
        return Ok(if tail == "/" {
            format!("/mnt/{drive}")
        } else {
            format!("/mnt/{drive}{tail}")
        });
    }
    if norm.starts_with('/') {
        // Keep Linux paths verbatim apart from a trailing slash.
        let t = if norm.len() > 1 {
            norm.trim_end_matches('/')
        } else {
            &norm
        };
        return Ok(if t.is_empty() {
            "/".into()
        } else {
            t.to_string()
        });
    }
    if norm == "~" || norm.starts_with("~/") {
        return Ok(norm.trim_end_matches('/').to_string());
    }
    Err(format!("`{path}` is not an absolute path"))
}

/// The inverse: the Windows path under which `linux` is visible (for the folder picker).
pub fn linux_to_windows(linux: &str, distro: &str) -> Result<String, String> {
    if !linux.starts_with('/') {
        return Err(format!("`{linux}` is not an absolute Linux path"));
    }
    let tail = collapse(linux);
    if let Some(rest) = tail.strip_prefix("/mnt/") {
        let mut it = rest.splitn(2, '/');
        let drive = it.next().unwrap_or("");
        if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
            let sub = it.next().unwrap_or("").replace('/', "\\");
            return Ok(format!("{}:\\{sub}", drive.to_ascii_uppercase()));
        }
    }
    Ok(format!(
        r"\\wsl.localhost\{distro}{}",
        if tail == "/" {
            String::new()
        } else {
            tail.replace('/', "\\")
        }
    ))
}

// ---- scripts ------------------------------------------------------------------------------

/// Quote for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,~".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Script run by `sh -lc` that starts the agent. `rc` defaults to the one on `PATH`.
pub fn agent_script(rc: Option<&str>) -> String {
    format!("{PATH_PREFIX}; exec {} acp", sh_quote(rc.unwrap_or("rc")))
}

/// Argument vector for `wsl.exe` that runs the agent in `distro`, started in `linux_cwd`.
pub fn agent_args(distro: &str, linux_cwd: &str, rc: Option<&str>) -> Vec<String> {
    ["-d", distro, "--cd", linux_cwd, "--", "sh", "-lc"]
        .iter()
        .map(|s| s.to_string())
        .chain([agent_script(rc)])
        .collect()
}

/// Script for `rc --version` detection.
pub fn check_script() -> String {
    format!("{PATH_PREFIX}; command -v rc && rc --version")
}

/// Standard base64 (RFC 4648), used to ship a multi-line script through `wsl.exe`'s command line
/// without any quoting concerns.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `sh -lc` payload that decodes and runs `script`.
pub fn encode_script(script: &str) -> String {
    format!("echo {} | base64 -d | sh", base64(script.as_bytes()))
}

/// Versions end up in a URL and a shell script: allow only release-tag characters.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 40
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
}

/// The POSIX shell script that installs `rc` into `~/.local/bin`. It detects the architecture,
/// downloads `rc-<version>-<target>.tar.gz` and its `.sha256` from the GitHub release `v<version>`
/// (falling back to the latest release when that tag has no such asset), verifies the checksum
/// and installs. Progress goes to stdout as `[rc-install] ...` lines.
pub fn build_install_script(version: &str, repo: &str) -> Result<String, String> {
    if !valid_version(version) {
        return Err(format!("invalid version `{version}`"));
    }
    if !repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
    {
        return Err(format!("invalid repository `{repo}`"));
    }
    Ok(format!(
        r#"set -eu
say() {{ printf '[rc-install] %s\n' "$*"; }}
repo='{repo}'
ver='{version}'
arch=$(uname -m)
case "$arch" in
  x86_64|amd64) target=x86_64-unknown-linux-musl ;;
  aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
  *) say "error: unsupported architecture $arch"; exit 2 ;;
esac
say "architecture $arch, target $target"
if command -v curl >/dev/null 2>&1; then
  fetch() {{ curl -fsSL --retry 2 -o "$2" "$1"; }}
elif command -v wget >/dev/null 2>&1; then
  fetch() {{ wget -q -O "$2" "$1"; }}
else
  say "error: neither curl nor wget is installed in this distro"; exit 3
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
base="https://github.com/$repo/releases/download"
say "downloading rc-$ver-$target.tar.gz"
if ! fetch "$base/v$ver/rc-$ver-$target.tar.gz" "$tmp/rc.tar.gz"; then
  say "release v$ver has no such asset; looking up the latest release"
  fetch "https://api.github.com/repos/$repo/releases/latest" "$tmp/latest.json" || {{ say "error: cannot query the latest release"; exit 5; }}
  ver=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{{0,1\}}\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)
  [ -n "$ver" ] || {{ say "error: could not determine the latest release"; exit 5; }}
  say "downloading rc-$ver-$target.tar.gz (latest)"
  fetch "$base/v$ver/rc-$ver-$target.tar.gz" "$tmp/rc.tar.gz" || {{ say "error: download failed"; exit 5; }}
fi
say "verifying checksum"
fetch "$base/v$ver/rc-$ver-$target.tar.gz.sha256" "$tmp/rc.sha256" || {{ say "error: checksum download failed"; exit 5; }}
want=$(awk '{{ print $1 }}' "$tmp/rc.sha256" | head -n 1)
if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$tmp/rc.tar.gz" | awk '{{ print $1 }}')
elif command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$tmp/rc.tar.gz" | awk '{{ print $1 }}')
else say "error: no sha256sum available"; exit 3; fi
[ -n "$want" ] && [ "$want" = "$got" ] || {{ say "error: checksum mismatch (expected $want, got $got)"; exit 4; }}
say "extracting"
tar -xzf "$tmp/rc.tar.gz" -C "$tmp"
bin=$(find "$tmp" -type f -name rc | head -n 1)
[ -n "$bin" ] || {{ say "error: archive does not contain rc"; exit 5; }}
mkdir -p "$HOME/.local/bin"
cp "$bin" "$HOME/.local/bin/rc.new"
chmod 755 "$HOME/.local/bin/rc.new"
mv -f "$HOME/.local/bin/rc.new" "$HOME/.local/bin/rc"
say "installed $HOME/.local/bin/rc (version $ver)"
"#
    ))
}

// ---- running wsl.exe ----------------------------------------------------------------------

/// A `wsl.exe` command that never flashes a console window.
pub fn wsl_command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    hide_window(&mut cmd);
    cmd
}

#[cfg(windows)]
pub fn hide_window(cmd: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_window(_cmd: &mut tokio::process::Command) {}

fn require_windows() -> Result<&'static str, String> {
    if cfg!(windows) {
        Ok("wsl.exe")
    } else {
        Err("WSL2 is only available on Windows".into())
    }
}

struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
}

async fn run(program: &str, args: &[String], timeout: Duration) -> Result<Output, String> {
    let mut cmd = wsl_command(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let fut = cmd.output();
    let out = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("`{program}` timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("failed to run `{program}`: {e}"))?;
    let mut stdout = out.stdout;
    stdout.extend_from_slice(&out.stderr);
    Ok(Output {
        code: out.status.code(),
        stdout,
    })
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// `wsl.exe -l -v`, falling back to `-l -q` on older builds.
pub async fn list_distros() -> Result<Vec<Distro>, String> {
    let prog = require_windows()?;
    let verbose = run(prog, &strs(&["-l", "-v"]), Duration::from_secs(15)).await?;
    if verbose.code == Some(0) {
        let found = parse_distro_list(&decode_wsl_output(&verbose.stdout));
        if !found.is_empty() {
            return Ok(found);
        }
    }
    let quiet = run(prog, &strs(&["-l", "-q"]), Duration::from_secs(15)).await?;
    let text = decode_wsl_output(&quiet.stdout);
    if quiet.code != Some(0) {
        return Err(format!("wsl.exe -l failed: {}", text.trim()));
    }
    Ok(parse_distro_names(&text))
}

fn sh_args(distro: &str, script: &str) -> Vec<String> {
    ["-d", distro, "--", "sh", "-lc"]
        .iter()
        .map(|s| s.to_string())
        .chain([script.to_string()])
        .collect()
}

pub async fn check_rc(distro: &str) -> Result<RcStatus, String> {
    check_rc_with(require_windows()?, distro).await
}

pub async fn check_rc_with(program: &str, distro: &str) -> Result<RcStatus, String> {
    let out = run(
        program,
        &sh_args(distro, &check_script()),
        Duration::from_secs(60),
    )
    .await?;
    let text = decode_wsl_output(&out.stdout);
    let expected = env!("CARGO_PKG_VERSION");
    match out.code {
        Some(0) => Ok(parse_rc_check(&text, expected)),
        // `command -v rc` failed inside the distro: not installed.
        Some(1) if !text.contains('/') => Ok(parse_rc_check("", expected)),
        _ => Err(format!("could not run rc in `{distro}`: {}", text.trim())),
    }
}

/// Run the install script, reporting each output line to `progress`.
pub async fn install_rc(distro: &str, progress: impl Fn(String)) -> Result<RcStatus, String> {
    install_rc_with(
        require_windows()?,
        distro,
        env!("CARGO_PKG_VERSION"),
        progress,
    )
    .await
}

pub async fn install_rc_with(
    program: &str,
    distro: &str,
    version: &str,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    let script = build_install_script(version, RELEASE_REPO)?;
    let mut cmd = wsl_command(program);
    cmd.args(sh_args(distro, &encode_script(&script)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run `{program}`: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let err_task = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b).await;
        b
    });
    // The script's own output is UTF-8; wsl.exe's messages (UTF-16) only appear on failure.
    let mut raw = Vec::new();
    let mut reader = BufReader::new(&mut stdout);
    let mut line = Vec::new();
    while reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(|e| e.to_string())?
        > 0
    {
        raw.extend_from_slice(&line);
        let t = decode_wsl_output(&line);
        let t = t.trim();
        if !t.is_empty() {
            progress(t.to_string());
        }
        line.clear();
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let err = err_task.await.unwrap_or_default();
    if !status.success() {
        let text = decode_wsl_output(&[raw, err].concat());
        let last = text
            .lines()
            .rev()
            .find(|l| l.contains("error:"))
            .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("install failed");
        return Err(last
            .trim()
            .trim_start_matches("[rc-install]")
            .trim()
            .to_string());
    }
    check_rc_with(program, distro).await
}

/// Ask the distro itself (`wslpath -u`), which knows custom automount roots.
pub async fn wslpath(distro: &str, windows_path: &str) -> Option<String> {
    let prog = require_windows().ok()?;
    let out = run(
        prog,
        &[
            "-d".into(),
            distro.into(),
            "--exec".into(),
            "wslpath".into(),
            "-u".into(),
            windows_path.replace('\\', "/"),
        ],
        Duration::from_secs(15),
    )
    .await
    .ok()?;
    let text = decode_wsl_output(&out.stdout);
    let p = text.trim();
    (out.code == Some(0) && p.starts_with('/')).then(|| p.to_string())
}

/// Translate for use as the agent's working directory: the pure translation (which also enforces
/// the distro check) refined by `wslpath` for drive paths when available.
pub async fn resolve_linux_path(distro: &str, path: &str) -> Result<String, String> {
    let pure = windows_to_linux(path, distro)?;
    let t = path.trim();
    let is_drive = t.as_bytes().get(1) == Some(&b':');
    if is_drive {
        if let Some(p) = wslpath(distro, t).await {
            return Ok(p);
        }
    }
    Ok(pure)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str, bom: bool) -> Vec<u8> {
        let mut v = Vec::new();
        if bom {
            v.extend_from_slice(&[0xFF, 0xFE]);
        }
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    const VERBOSE: &str = "  NAME              STATE           VERSION\r\n* Ubuntu-24.04      Running         2\r\n  Debian            Stopped         2\r\n  old-one           Stopped         1\r\n  docker-desktop    Running         2\r\n";

    #[test]
    fn decodes_utf16_with_and_without_bom_and_utf8() {
        let want = "  NAME  STATE\n* Ubuntu  Running\n";
        let src = "  NAME  STATE\r\n* Ubuntu  Running\r\n";
        assert_eq!(decode_wsl_output(&utf16le(src, true)), want);
        assert_eq!(decode_wsl_output(&utf16le(src, false)), want);
        assert_eq!(decode_wsl_output(src.as_bytes()), want);
        assert_eq!(decode_wsl_output(b""), "");
        assert_eq!(
            decode_wsl_output("/home/u/.local/bin/rc\nrc 0.1.0\n".as_bytes()),
            "/home/u/.local/bin/rc\nrc 0.1.0\n"
        );
    }

    #[test]
    fn parses_verbose_list() {
        let text = decode_wsl_output(&utf16le(VERBOSE, true));
        let d = parse_distro_list(&text);
        assert_eq!(d.len(), 4);
        assert_eq!(d[0].name, "Ubuntu-24.04");
        assert!(d[0].default && d[0].selectable);
        assert_eq!(d[0].state.as_deref(), Some("Running"));
        assert_eq!(d[0].version, Some(2));
        assert!(!d[1].default && d[1].selectable);
        assert!(!d[2].selectable && d[2].reason.as_deref().unwrap().contains("WSL1"));
        assert!(!d[3].selectable && d[3].reason.as_deref().unwrap().contains("Docker"));
    }

    #[test]
    fn parses_localized_states_and_ignores_noise() {
        let text = "  NAME      ZUSTAND          VERSION\n* Ubuntu    Wird ausgef\u{fc}hrt    2\n\nSome warning line\n";
        let d = parse_distro_list(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].state.as_deref(), Some("Wird ausgef\u{fc}hrt"));
        assert!(parse_distro_list("").is_empty());
        assert!(
            parse_distro_list("Windows Subsystem for Linux has no installed distributions.")
                .is_empty()
        );
    }

    #[test]
    fn parses_quiet_list() {
        let text = decode_wsl_output(&utf16le("Ubuntu\r\nDebian\r\n\r\n", true));
        let d = parse_distro_names(&text);
        assert_eq!(
            d.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["Ubuntu", "Debian"]
        );
        assert!(d.iter().all(|d| d.selectable && d.version.is_none()));
    }

    #[test]
    fn parses_rc_check() {
        let s = parse_rc_check("/home/u/.local/bin/rc\nrc 0.1.0\n", "0.1.0");
        assert!(s.installed);
        assert_eq!(s.path.as_deref(), Some("/home/u/.local/bin/rc"));
        assert_eq!(s.version.as_deref(), Some("0.1.0"));
        let s = parse_rc_check("/usr/bin/rc\nrc v0.2.3-4-gabc (x86_64)\n", "0.1.0");
        assert_eq!(s.version.as_deref(), Some("0.2.3-4-gabc"));
        let s = parse_rc_check("", "0.1.0");
        assert!(!s.installed && s.version.is_none());
    }

    #[test]
    fn translates_unc_paths() {
        let d = "Ubuntu-24.04";
        assert_eq!(
            windows_to_linux(r"\\wsl$\Ubuntu-24.04\home\x", d).unwrap(),
            "/home/x"
        );
        assert_eq!(
            windows_to_linux(r"\\wsl.localhost\Ubuntu-24.04\home\x\", d).unwrap(),
            "/home/x"
        );
        assert_eq!(
            windows_to_linux(r"\\WSL.LOCALHOST\ubuntu-24.04\home\x", d).unwrap(),
            "/home/x"
        );
        assert_eq!(windows_to_linux(r"\\wsl$\Ubuntu-24.04", d).unwrap(), "/");
        assert_eq!(
            windows_to_linux("//wsl$/Ubuntu-24.04/a//b", d).unwrap(),
            "/a/b"
        );
        assert_eq!(
            windows_to_linux(r"\\?\UNC\wsl.localhost\Ubuntu-24.04\srv", d).unwrap(),
            "/srv"
        );
        let e = windows_to_linux(r"\\wsl$\Debian\home\x", d).unwrap_err();
        assert!(e.contains("Debian") && e.contains("Ubuntu-24.04"));
        assert!(windows_to_linux(r"\\wsl$", d).is_err());
        assert!(windows_to_linux(r"\\fileserver\share\x", d)
            .unwrap_err()
            .contains("network"));
    }

    #[test]
    fn translates_drive_paths() {
        let d = "U";
        assert_eq!(
            windows_to_linux(r"C:\Users\x\proj", d).unwrap(),
            "/mnt/c/Users/x/proj"
        );
        assert_eq!(
            windows_to_linux(r"c:\Users\x", d).unwrap(),
            "/mnt/c/Users/x"
        );
        assert_eq!(
            windows_to_linux("D:/work/a b/", d).unwrap(),
            "/mnt/d/work/a b"
        );
        assert_eq!(windows_to_linux(r"E:\", d).unwrap(), "/mnt/e");
        assert_eq!(windows_to_linux("e:", d).unwrap(), "/mnt/e");
        assert_eq!(windows_to_linux(r"\\?\C:\x", d).unwrap(), "/mnt/c/x");
        assert!(windows_to_linux(r"C:foo", d).is_err());
    }

    #[test]
    fn linux_paths_pass_through() {
        let d = "U";
        assert_eq!(windows_to_linux("/home/x/proj", d).unwrap(), "/home/x/proj");
        assert_eq!(
            windows_to_linux("/home/x/proj/", d).unwrap(),
            "/home/x/proj"
        );
        assert_eq!(windows_to_linux("/", d).unwrap(), "/");
        assert_eq!(windows_to_linux("  /mnt/c/x  ", d).unwrap(), "/mnt/c/x");
        assert_eq!(windows_to_linux("~/code", d).unwrap(), "~/code");
        assert_eq!(windows_to_linux("~", d).unwrap(), "~");
        assert!(windows_to_linux("relative/dir", d).is_err());
        assert!(windows_to_linux("   ", d).is_err());
    }

    #[test]
    fn linux_to_windows_roundtrips() {
        assert_eq!(
            linux_to_windows("/mnt/c/Users/x", "U").unwrap(),
            r"C:\Users\x"
        );
        assert_eq!(linux_to_windows("/mnt/d", "U").unwrap(), r"D:\");
        assert_eq!(
            linux_to_windows("/home/x", "Ubuntu").unwrap(),
            r"\\wsl.localhost\Ubuntu\home\x"
        );
        assert_eq!(
            linux_to_windows("/", "Ubuntu").unwrap(),
            r"\\wsl.localhost\Ubuntu"
        );
        assert_eq!(
            linux_to_windows("/mnt/wsl/x", "U").unwrap(),
            r"\\wsl.localhost\U\mnt\wsl\x"
        );
        assert!(linux_to_windows("rel", "U").is_err());
        for p in ["/home/x/p", "/mnt/c/a/b", "/opt"] {
            let w = linux_to_windows(p, "U").unwrap();
            assert_eq!(windows_to_linux(&w, "U").unwrap(), p);
        }
    }

    #[test]
    fn builds_agent_command() {
        assert_eq!(
            agent_script(None),
            "PATH=$HOME/.local/bin:$PATH; exec rc acp"
        );
        assert_eq!(
            agent_script(Some("/opt/my rc/rc")),
            "PATH=$HOME/.local/bin:$PATH; exec '/opt/my rc/rc' acp"
        );
        let a = agent_args("Ubuntu", "/home/x", None);
        assert_eq!(
            &a[..8],
            [
                "-d",
                "Ubuntu",
                "--cd",
                "/home/x",
                "--",
                "sh",
                "-lc",
                &agent_script(None)
            ]
        );
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
        assert!(encode_script("echo hi\n").starts_with("echo ZWNobyBoaQo= |"));
    }

    #[test]
    fn install_script_contents_and_validation() {
        let s = build_install_script("0.1.0", RELEASE_REPO).unwrap();
        assert!(s.contains("ver='0.1.0'") && s.contains("repo='ruckc/ruckcode'"));
        assert!(s.contains("x86_64-unknown-linux-musl") && s.contains("aarch64-unknown-linux-gnu"));
        assert!(s.contains("uname -m") && s.contains("sha256") && s.contains(".local/bin"));
        assert!(s.contains("releases/download") && s.contains("releases/latest"));
        for bad in ["", "1.0; rm -rf /", "a b", "$(x)", "1'2"] {
            assert!(build_install_script(bad, RELEASE_REPO).is_err(), "{bad}");
        }
        assert!(build_install_script("1.0.0", "a/b'c").is_err());
    }

    fn have(tool: &str) -> bool {
        std::process::Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn install_script_is_valid_posix_sh() {
        if !have("sh") {
            return;
        }
        let s = build_install_script("0.1.0", RELEASE_REPO).unwrap();
        let dir = std::env::temp_dir().join(format!("rc-wsl-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("install.sh");
        std::fs::write(&f, s).unwrap();
        let out = std::process::Command::new("sh")
            .arg("-n")
            .arg(&f)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs the real script with a fake `curl` (no network): a good checksum installs, a bad one
    /// is rejected, and a missing `v<ver>` asset falls back to the latest release.
    #[cfg(unix)]
    #[test]
    fn install_script_runs_against_fake_curl() {
        use std::os::unix::fs::PermissionsExt;
        if !have("sh") || !have("tar") || !have("sha256sum") {
            return;
        }
        let root = std::env::temp_dir().join(format!("rc-wsl-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bin, home, fixtures) = (root.join("bin"), root.join("home"), root.join("fx"));
        for d in [&bin, &home, &fixtures, &root.join("pkg")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let rc = root.join("pkg/rc");
        std::fs::write(&rc, "#!/bin/sh\necho rc 9.9.9\n").unwrap();
        let sh = |cmd: &str| {
            let o = std::process::Command::new("sh")
                .args(["-c", cmd])
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{cmd}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        let arch = String::from_utf8(
            std::process::Command::new("uname")
                .arg("-m")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let target = match arch.trim() {
            "x86_64" => "x86_64-unknown-linux-musl",
            "aarch64" => "aarch64-unknown-linux-gnu",
            _ => return,
        };
        let fx = fixtures.display();
        // Only v9.9.9 exists; "latest" points at it.
        sh(&format!(
            "cd {} && tar -czf {fx}/rc-9.9.9-{target}.tar.gz rc",
            root.join("pkg").display()
        ));
        sh(&format!(
            "cd {fx} && sha256sum rc-9.9.9-{target}.tar.gz > rc-9.9.9-{target}.tar.gz.sha256"
        ));
        std::fs::write(fixtures.join("latest.json"), r#"{"tag_name": "v9.9.9"}"#).unwrap();
        let curl = bin.join("curl");
        std::fs::write(
            &curl,
            format!(
                r#"#!/bin/sh
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; -*) shift;; *) url="$1"; shift;; esac; done
f="{fx}/$(basename "$url")"
case "$url" in *releases/latest) f="{fx}/latest.json";; esac
[ -f "$f" ] || exit 22
cp "$f" "$out"
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run_script = |ver: &str| {
            let script = build_install_script(ver, RELEASE_REPO).unwrap();
            std::process::Command::new("sh")
                .args(["-c", &script])
                .env("HOME", &home)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .output()
                .unwrap()
        };
        // Falls back from the missing 0.1.0 tag to latest (9.9.9) and installs.
        let out = run_script("0.1.0");
        let log = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{log}");
        assert!(
            log.contains("looking up the latest") && log.contains("installed"),
            "{log}"
        );
        assert!(home.join(".local/bin/rc").exists());
        // Exact version works directly.
        assert!(run_script("9.9.9").status.success());
        // Tampered checksum is rejected.
        std::fs::write(
            fixtures.join(format!("rc-9.9.9-{target}.tar.gz.sha256")),
            "00  x\n",
        )
        .unwrap();
        let out = run_script("9.9.9");
        assert_eq!(out.status.code(), Some(4));
        assert!(String::from_utf8_lossy(&out.stdout).contains("checksum mismatch"));
        let _ = std::fs::remove_dir_all(root);
    }
}
