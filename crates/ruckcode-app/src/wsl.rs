//! WSL2 support: distro discovery, `rc` provisioning inside a distro and path translation.
//!
//! Everything that parses or builds text is a pure function so it is testable on any OS; only the
//! thin `run_wsl*` helpers actually launch `wsl.exe`, and those refuse to run off Windows.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

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

impl RcStatus {
    /// `rc` is present and reports exactly the app's version.
    pub fn is_current(&self) -> bool {
        self.installed && self.version.as_deref() == Some(self.expected.as_str())
    }
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
    exec_args(distro, Some(linux_cwd), &agent_script(rc))
}

/// Argument vector `wsl.exe -d <distro> [--cd <dir>] --exec sh -lc <wrapped script>`.
///
/// `--exec` is essential: after a plain `--`, wsl.exe joins the remaining arguments into ONE
/// command line and has the distro's default shell parse it again, which splits our script and
/// lets `(`, `;`, quotes ... reach `/bin/sh` unquoted (`sh: 1: Syntax error: "(" unexpected`).
/// With `--exec` the program is started directly with exactly these arguments. The script itself
/// is additionally base64-wrapped (see [`wrap_script`]), so the argument carries no user text.
pub fn exec_args(distro: &str, linux_cwd: Option<&str>, script: &str) -> Vec<String> {
    let mut v = vec!["-d".to_string(), distro.to_string()];
    if let Some(dir) = linux_cwd {
        v.push("--cd".into());
        v.push(dir.to_string());
    }
    v.extend(["--exec", "sh", "-lc"].map(String::from));
    v.push(wrap_script(script));
    v
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

/// `sh -lc` payload that decodes and runs `script` in the SAME shell (so stdin and `exec` keep
/// working, unlike a `| sh` pipe). The argument only contains `[A-Za-z0-9+/=]`, spaces and the
/// fixed wrapper punctuation `;` `$(` `)` `|` (no quotes or backslashes, nothing Windows command
/// line quoting would need to escape). `set -f` plus an empty `IFS` keep the unquoted command
/// substitution a single, unglobbed word for `eval`; the payload restores both first. If
/// `base64` is missing, `exit 127` is evaluated instead of silently running nothing.
pub fn wrap_script(script: &str) -> String {
    let payload = format!("unset IFS; set +f\n{script}");
    format!(
        "set -f; IFS=; eval $(printf %s {} | base64 -d || echo exit 127)",
        base64(payload.as_bytes())
    )
}

/// Versions end up in a URL and a shell script: allow only release-tag characters.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 40
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
}

/// The shared installer (`site/install.sh`, also published on the download site as `install.sh`).
const INSTALL_TEMPLATE: &str = include_str!("../../../site/install.sh");

fn valid_base_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 200
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || ":/._~-".contains(c))
}

/// The POSIX shell script that installs `rc` into `~/.local/bin`: the site's `install.sh` with
/// the site URL, the updater public key and a fallback version filled in. It resolves the latest
/// release from `<base>/rc/latest.json` (only the latest version is hosted), downloads the
/// archive, verifies the SHA-256 and, when `minisign` exists in the distro and a key is
/// configured, the minisign signature. Progress goes to stdout as `[rc-install] ...` lines and
/// failures end with `[rc-install] error: ...`.
pub fn build_install_script(
    base_url: &str,
    pubkey: Option<&str>,
    fallback_version: &str,
) -> Result<String, String> {
    if !valid_base_url(base_url) {
        return Err(format!("invalid update site URL `{base_url}`"));
    }
    let pubkey = pubkey.map(str::trim).unwrap_or("");
    if !pubkey
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
    {
        return Err("invalid updater public key".into());
    }
    if !valid_version(fallback_version) {
        return Err(format!("invalid version `{fallback_version}`"));
    }
    Ok(INSTALL_TEMPLATE
        .replace("@BASE_URL@", base_url.trim_end_matches('/'))
        .replace("@PUBKEY@", pubkey)
        .replace("@VERSION@", fallback_version))
}

// ---- bundled rc -----------------------------------------------------------------------------

/// Path of the Linux `rc` inside the app's resource directory (Windows installers only).
pub const BUNDLED_RESOURCE: &str = "wsl/rc";

/// Where an `rc` install comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RcSource {
    /// The static x86_64 binary shipped in the installer, streamed into the distro.
    Bundled(PathBuf),
    /// Download from the update site (dev builds without the resource, other architectures).
    Pages,
}

/// The bundled binary is a static x86_64 musl build, so it only fits x86_64 distros and must
/// exist on disk (dev and non-Windows builds have no such resource).
pub fn choose_source(bundled: Option<PathBuf>, arch: &str) -> RcSource {
    match bundled {
        Some(p) if arch.trim() == "x86_64" && p.is_file() => RcSource::Bundled(p),
        _ => RcSource::Pages,
    }
}

/// SHA-256 (FIPS 180-4) as lowercase hex; used to verify the copy inside the distro.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finish_hex()
}

pub struct Sha256 {
    state: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: Vec::with_capacity(64),
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        while !data.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let block: [u8; 64] = self.buf[..].try_into().unwrap();
                self.compress(&block);
                self.buf.clear();
            }
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = self.state;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (s, x) in self.state.iter_mut().zip(v) {
            *s = s.wrapping_add(x);
        }
    }

    pub fn finish_hex(mut self) -> String {
        let bits = self.len * 8;
        let mut pad = vec![0x80u8];
        while (self.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_be_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        self.state.iter().map(|x| format!("{x:08x}")).collect()
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// POSIX `sh` script that receives the `rc` binary on STDIN (no network, no UNC paths) and
/// installs it as `~/.local/bin/rc`: write to `rc.tmp`, verify the SHA-256 when `sha256sum`
/// exists, `chmod 0755`, atomically `mv` over `rc`. `sha256` is the host-computed digest.
pub fn stdin_copy_script(sha256: &str) -> Result<String, String> {
    if sha256.len() != 64 || !sha256.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err("invalid sha256".into());
    }
    Ok(format!(
        r#"d="$HOME/.local/bin"
mkdir -p "$d" || {{ echo "[rc-install] error: cannot create $d"; exit 2; }}
rm -f "$d/rc.tmp"
cat > "$d/rc.tmp" || {{ echo "[rc-install] error: cannot write $d/rc.tmp"; rm -f "$d/rc.tmp"; exit 2; }}
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$d/rc.tmp" | cut -d' ' -f1)
  if [ "$got" != "{sha256}" ]; then
    echo "[rc-install] error: checksum mismatch after copy (got $got)"
    rm -f "$d/rc.tmp"
    exit 4
  fi
  echo "[rc-install] sha256 {sha256} verified"
else
  echo "[rc-install] sha256sum not found in the distro; checksum not verified"
fi
chmod 0755 "$d/rc.tmp" && mv -f "$d/rc.tmp" "$d/rc" || {{ echo "[rc-install] error: cannot install $d/rc"; rm -f "$d/rc.tmp"; exit 2; }}
echo "[rc-install] installed $d/rc"
"#
    ))
}

#[cfg(test)]
pub(crate) fn b64_decode_for_tests(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    };
    let bytes: Vec<u8> = s.bytes().filter(|b| *b != b'=').collect();
    let mut out = Vec::new();
    for c in bytes.chunks(4) {
        let n = c.iter().fold(0u32, |a, b| a << 6 | val(*b) as u32) << (6 * (4 - c.len()));
        let b = n.to_be_bytes();
        out.extend_from_slice(&b[1..c.len()]);
    }
    out
}

/// Test helper: the user script inside a [`wrap_script`] argument (without the preamble).
#[cfg(test)]
pub(crate) fn unwrap_script_for_tests(arg: &str) -> String {
    let rest = arg
        .strip_prefix("set -f; IFS=; eval $(printf %s ")
        .and_then(|r| r.strip_suffix(" | base64 -d || echo exit 127)"))
        .expect("wrapped script shape");
    let text = String::from_utf8(b64_decode_for_tests(rest)).unwrap();
    text.strip_prefix("unset IFS; set +f\n")
        .unwrap()
        .to_string()
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
    exec_args(distro, None, script)
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
    let expected = crate::app_version();
    match out.code {
        Some(0) => return Ok(parse_rc_check(&text, expected)),
        // `command -v rc` failed inside the distro: not installed.
        Some(1) if !text.contains('/') => return Ok(parse_rc_check("", expected)),
        _ => {}
    }
    // The wrapped check failed in an unexpected way. Probe the usual install location directly,
    // without a login shell or the base64 wrapper, before giving up.
    let fallback = run(
        program,
        &strs(&["-d", distro, "--exec", "sh", "-c", FALLBACK_CHECK]),
        Duration::from_secs(60),
    )
    .await;
    if let Ok(fb) = &fallback {
        let fb_text = decode_wsl_output(&fb.stdout);
        if fb.code == Some(0) {
            return Ok(parse_rc_check(&fb_text, expected));
        }
    }
    Err(check_failure(
        distro,
        out.code,
        &text,
        fallback.ok().as_ref(),
    ))
}

/// Direct probe of `~/.local/bin/rc` used when the wrapped check fails; contains no characters
/// that any shell layer could re-parse differently.
const FALLBACK_CHECK: &str =
    "command -v rc || ls $HOME/.local/bin/rc && $HOME/.local/bin/rc --version";

fn check_failure(distro: &str, code: Option<i32>, text: &str, fallback: Option<&Output>) -> String {
    let code = code.map_or_else(|| "none".to_string(), |c| c.to_string());
    let text = text.trim();
    let mut msg = format!(
        "could not run rc in `{distro}` (exit code {code}): {}",
        if text.is_empty() { "no output" } else { text }
    );
    if let Some(fb) = fallback {
        let fb_text = decode_wsl_output(&fb.stdout);
        let fb_text = fb_text.trim();
        msg.push_str(&format!(
            "; direct probe exit code {}: {}",
            fb.code
                .map_or_else(|| "none".to_string(), |c| c.to_string()),
            if fb_text.is_empty() {
                "no output"
            } else {
                fb_text
            }
        ));
    }
    msg
}

/// Run the install script (latest `rc` from the update site), reporting each output line to
/// `progress`. `pubkey` is the updater public key, if this build has one.
pub async fn install_rc(
    distro: &str,
    pubkey: Option<&str>,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    install_rc_with(
        require_windows()?,
        distro,
        &crate::update::base_url(),
        pubkey,
        crate::app_version(),
        progress,
    )
    .await
}

pub async fn install_rc_with(
    program: &str,
    distro: &str,
    base_url: &str,
    pubkey: Option<&str>,
    fallback_version: &str,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    let script = build_install_script(base_url, pubkey, fallback_version)?;
    run_install_script(program, distro, &script, None, &progress).await?;
    check_rc_with(program, distro).await
}

/// `uname -m` of the distro (`x86_64`, `aarch64`, ...).
pub async fn distro_arch(program: &str, distro: &str) -> Result<String, String> {
    let out = run(
        program,
        &sh_args(distro, "uname -m"),
        Duration::from_secs(30),
    )
    .await?;
    let text = decode_wsl_output(&out.stdout);
    match out.code {
        Some(0) => Ok(text.lines().last().unwrap_or("").trim().to_string()),
        _ => Err(format!(
            "could not run uname in `{distro}`: {}",
            text.trim()
        )),
    }
}

/// Install `rc` into the distro: the bundled binary streamed over stdin when `bundled` exists
/// and the distro is x86_64, otherwise the latest release from the update site. Ends with a
/// `rc --version` check; a bundled install must report exactly the app version.
pub async fn install_bundled_rc_with(
    program: &str,
    distro: &str,
    bundled: Option<PathBuf>,
    base_url: &str,
    pubkey: Option<&str>,
    fallback_version: &str,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    let arch = if bundled.is_some() {
        distro_arch(program, distro).await?
    } else {
        String::new()
    };
    let path = match choose_source(bundled, &arch) {
        RcSource::Bundled(p) => p,
        RcSource::Pages => {
            progress("bundled rc not usable here; downloading from the update site".into());
            return install_rc_with(
                program,
                distro,
                base_url,
                pubkey,
                fallback_version,
                progress,
            )
            .await;
        }
    };
    let data = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("cannot read bundled rc `{}`: {e}", path.display()))?;
    let sha = sha256_hex(&data);
    progress(format!(
        "[rc-install] copying bundled rc ({} KiB, sha256 {}) into {distro}",
        data.len() / 1024,
        &sha[..12]
    ));
    let script = stdin_copy_script(&sha)?;
    run_install_script(program, distro, &script, Some(data), &progress).await?;
    let status = check_rc_with(program, distro).await?;
    if !status.is_current() {
        return Err(format!(
            "installed rc reports {}, expected {}",
            status.version.as_deref().unwrap_or("no version"),
            status.expected
        ));
    }
    Ok(status)
}

/// Check `rc` in the distro and install the bundled copy when it is missing or its version
/// differs from the app's. Returns the resulting status.
pub async fn ensure_rc_with(
    program: &str,
    distro: &str,
    bundled: Option<PathBuf>,
    base_url: &str,
    pubkey: Option<&str>,
    fallback_version: &str,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    let status = check_rc_with(program, distro).await?;
    if status.is_current() {
        return Ok(status);
    }
    install_bundled_rc_with(
        program,
        distro,
        bundled,
        base_url,
        pubkey,
        fallback_version,
        progress,
    )
    .await
}

/// Run `script` in the distro, optionally streaming `stdin_data` into it, and report each
/// output line to `progress`.
async fn run_install_script(
    program: &str,
    distro: &str,
    script: &str,
    stdin_data: Option<Vec<u8>>,
    progress: &impl Fn(String),
) -> Result<(), String> {
    let mut cmd = wsl_command(program);
    cmd.args(sh_args(distro, script))
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run `{program}`: {e}"))?;
    if let Some(data) = stdin_data {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        // A failed write (the script exited early) surfaces through its own error line below.
        tokio::spawn(async move {
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, &data).await;
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut stdin).await;
        });
    }
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
    Ok(())
}

/// Production entry points: `wsl.exe`, the app's update site and key.
pub async fn install_bundled_rc(
    distro: &str,
    bundled: Option<PathBuf>,
    pubkey: Option<&str>,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    install_bundled_rc_with(
        require_windows()?,
        distro,
        bundled,
        &crate::update::base_url(),
        pubkey,
        crate::app_version(),
        progress,
    )
    .await
}

pub async fn ensure_rc(
    distro: &str,
    bundled: Option<PathBuf>,
    pubkey: Option<&str>,
    progress: impl Fn(String),
) -> Result<RcStatus, String> {
    ensure_rc_with(
        require_windows()?,
        distro,
        bundled,
        &crate::update::base_url(),
        pubkey,
        crate::app_version(),
        progress,
    )
    .await
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
    // `rc` gets the path as plain data (no shell), so `~` must be expanded here: left as is it
    // becomes a relative `./~/...` directory under whatever the agent's cwd happens to be.
    if pure == "~" || pure.starts_with("~/") {
        let home = linux_home(distro).await?;
        return Ok(expand_home(&pure, &home));
    }
    Ok(pure)
}

/// `$HOME` inside `distro`.
async fn linux_home(distro: &str) -> Result<String, String> {
    let prog = require_windows()?;
    let out = run(
        prog,
        &strs(&["-d", distro, "--exec", "sh", "-c", "printf %s \"$HOME\""]),
        Duration::from_secs(15),
    )
    .await?;
    let text = decode_wsl_output(&out.stdout);
    let home = text.trim();
    if out.code == Some(0) && home.starts_with('/') {
        Ok(home.to_string())
    } else {
        Err(format!(
            "could not find the home folder in `{distro}`: {home}"
        ))
    }
}

fn expand_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => path.to_string(),
    }
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
    fn expands_home() {
        assert_eq!(expand_home("~", "/home/u"), "/home/u");
        assert_eq!(expand_home("~/a/b", "/home/u/"), "/home/u/a/b");
        assert_eq!(expand_home("/x", "/home/u"), "/x");
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
            &a[..7],
            ["-d", "Ubuntu", "--cd", "/home/x", "--exec", "sh", "-lc"]
        );
        assert_eq!(unwrap_script_for_tests(&a[7]), agent_script(None));
        assert_eq!(a.len(), 8);
        assert!(!a.iter().any(|x| x == "--"));
        // No --cd without a directory.
        let b = exec_args("Ubuntu-26.04", None, "true");
        assert_eq!(&b[..5], ["-d", "Ubuntu-26.04", "--exec", "sh", "-lc"]);
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
    }

    const BASE: &str = "https://example.test/ruckcode";

    #[test]
    fn install_script_contents_and_validation() {
        let s = build_install_script(BASE, Some("QUJD+/=="), "0.1.0").unwrap();
        assert!(
            s.contains("BASE_URL=\"${RUCKCODE_UPDATE_BASE_URL:-https://example.test/ruckcode}\"")
        );
        assert!(s.contains("PUBKEY='QUJD+/=='") && s.contains("DEFAULT_VERSION='0.1.0'"));
        assert!(s.contains("x86_64-unknown-linux-musl") && s.contains("aarch64-unknown-linux-gnu"));
        assert!(s.contains("rc/latest.json") && s.contains("sha256") && s.contains(".local/bin"));
        assert!(!s.contains("github.com") && !s.contains("@BASE_URL@") && !s.contains("@VERSION@"));
        // No key: the placeholder becomes empty (checksum only).
        assert!(build_install_script(BASE, None, "0.1.0")
            .unwrap()
            .contains("PUBKEY=''"));
        for bad in ["", "1.0; rm -rf /", "a b", "$(x)", "1'2"] {
            assert!(build_install_script(BASE, None, bad).is_err(), "{bad}");
        }
        for bad in [
            "",
            "ftp://x",
            "https://a/b'c",
            "https://a b",
            "https://x/$(id)",
        ] {
            assert!(build_install_script(bad, None, "1.0.0").is_err(), "{bad}");
        }
        assert!(build_install_script(BASE, Some("a'b"), "1.0.0").is_err());
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
        let s = build_install_script(BASE, Some("QUJD"), "0.1.0").unwrap();
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

    /// Runs the real script with a fake `curl` (no network) against a site tree: the latest
    /// version comes from `rc/latest.json`, the checksum is verified, and with a configured key
    /// the (fake) `minisign` decides about the signature.
    #[cfg(unix)]
    #[test]
    fn install_script_runs_against_fake_site() {
        use std::os::unix::fs::PermissionsExt;
        if !have("sh") || !have("tar") || !have("sha256sum") {
            return;
        }
        let root = std::env::temp_dir().join(format!("rc-wsl-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bin, home, site) = (root.join("bin"), root.join("home"), root.join("site"));
        let dl = site.join("download/v9.9.9");
        for d in [&bin, &home, &dl, &site.join("rc"), &root.join("pkg")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(root.join("pkg/rc"), "#!/bin/sh\necho rc 9.9.9\n").unwrap();
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
        let archive = format!("rc-9.9.9-{target}.tar.gz");
        sh(&format!(
            "cd {} && tar -czf {}/{archive} rc && cd {} && sha256sum {archive} > {archive}.sha256 && echo c2ln > {archive}.sig",
            root.join("pkg").display(),
            dl.display(),
            dl.display()
        ));
        std::fs::write(
            site.join("rc/latest.json"),
            "{\n  \"version\": \"9.9.9\",\n  \"targets\": {}\n}\n",
        )
        .unwrap();
        let write_exe = |name: &str, body: &str| {
            let f = bin.join(name);
            std::fs::write(&f, body).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write_exe(
            "curl",
            &format!(
                r#"#!/bin/sh
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; -*) shift;; *) url="$1"; shift;; esac; done
f="{}/${{url#{BASE}/}}"
[ -f "$f" ] || exit 22
cp "$f" "$out"
"#,
                site.display()
            ),
        );
        let run_script = |script: String| {
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
        let plain = || build_install_script(BASE, None, "0.1.0").unwrap();
        // Latest (9.9.9) from the manifest, checksum only.
        let out = run_script(plain());
        let log = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{log}");
        assert!(
            log.contains("publishes no signing key") && log.contains("installed"),
            "{log}"
        );
        assert!(log.contains("version 9.9.9"), "{log}");
        assert!(home.join(".local/bin/rc").exists());
        // Manifest missing: falls back to the baked-in version.
        std::fs::remove_file(site.join("rc/latest.json")).unwrap();
        let out = run_script(build_install_script(BASE, None, "9.9.9").unwrap());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let out = run_script(plain());
        assert_eq!(
            out.status.code(),
            Some(5),
            "unknown version has no download"
        );
        // With a key: a fake minisign that accepts, then one that rejects.
        write_exe("minisign", "#!/bin/sh\nexit 0\n");
        let keyed = || build_install_script(BASE, Some("QUJD"), "9.9.9").unwrap();
        let out = run_script(keyed());
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("signature OK"),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        write_exe("minisign", "#!/bin/sh\nexit 1\n");
        std::fs::remove_file(home.join(".local/bin/rc")).unwrap();
        let out = run_script(keyed());
        assert_eq!(out.status.code(), Some(6));
        assert!(!home.join(".local/bin/rc").exists(), "nothing installed");
        // Tampered checksum is rejected.
        std::fs::write(dl.join(format!("{archive}.sha256")), "00  x\n").unwrap();
        let out = run_script(plain_with_version());
        assert_eq!(out.status.code(), Some(4));
        assert!(String::from_utf8_lossy(&out.stdout).contains("checksum mismatch"));
        let _ = std::fs::remove_dir_all(root);

        fn plain_with_version() -> String {
            build_install_script(BASE, None, "9.9.9").unwrap()
        }
    }

    const NASTY: &str = "echo \"a (b) $(echo hi)\" 'q' ; true && echo `x`\nif [ -n \"$HOME\" ]; then\n  echo *.nothing \\ done\nfi\ncat <<'EOF'\nheredoc ( ) \"\nEOF\n";

    /// Windows command-line quoting as Rust's `Command` does it, and the inverse
    /// (`CommandLineToArgvW` rules) as `wsl.exe` applies it to its own command line.
    fn win_quote(args: &[String]) -> String {
        let mut out = Vec::new();
        for a in args {
            if !a.is_empty() && !a.contains([' ', '\t', '"']) {
                out.push(a.clone());
                continue;
            }
            let mut q = String::from("\"");
            let mut bs = 0;
            for c in a.chars() {
                match c {
                    '\\' => bs += 1,
                    '"' => {
                        q.push_str(&"\\".repeat(bs * 2 + 1));
                        q.push('"');
                        bs = 0;
                    }
                    _ => {
                        q.push_str(&"\\".repeat(bs));
                        bs = 0;
                        q.push(c);
                    }
                }
            }
            q.push_str(&"\\".repeat(bs * 2));
            q.push('"');
            out.push(q);
        }
        out.join(" ")
    }

    fn win_split(line: &str) -> Vec<String> {
        let (mut out, mut cur, mut inq, mut has) = (Vec::new(), String::new(), false, false);
        let cs: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            match cs[i] {
                '\\' => {
                    let mut n = 0;
                    while i < cs.len() && cs[i] == '\\' {
                        n += 1;
                        i += 1;
                    }
                    has = true;
                    if i < cs.len() && cs[i] == '"' {
                        cur.push_str(&"\\".repeat(n / 2));
                        if n % 2 == 1 {
                            cur.push('"');
                            i += 1;
                        }
                    } else {
                        cur.push_str(&"\\".repeat(n));
                    }
                    continue;
                }
                '"' => {
                    inq = !inq;
                    has = true;
                }
                c if c.is_whitespace() && !inq => {
                    if has {
                        out.push(std::mem::take(&mut cur));
                        has = false;
                    }
                }
                c => {
                    cur.push(c);
                    has = true;
                }
            }
            i += 1;
        }
        if has {
            out.push(cur);
        }
        out
    }

    #[test]
    fn wrapped_scripts_are_quoting_proof() {
        let a = exec_args("Ubuntu-26.04", Some("/home/u/my proj"), NASTY);
        assert_eq!(
            &a[..7],
            [
                "-d",
                "Ubuntu-26.04",
                "--cd",
                "/home/u/my proj",
                "--exec",
                "sh",
                "-lc"
            ]
        );
        assert!(!a.iter().any(|x| x == "--"));
        let w = &a[7];
        // Round trip.
        assert_eq!(unwrap_script_for_tests(w), NASTY);
        // The argument holds none of the script's own punctuation: only the fixed wrapper
        // plus one base64 token, and nothing Windows quoting or a re-parse could trip over.
        for bad in [
            '"', '\'', '\\', '\n', '\r', '&', '`', '<', '>', '*', '{', '}',
        ] {
            assert!(!w.contains(bad), "{bad:?} in {w}");
        }
        assert!(w
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/= ;$()|-%".contains(c)));
        assert!(!w.contains("&&") && !w.contains("echo hi") && !w.contains("heredoc"));
        let token = w.split(' ').find(|t| t.len() > 40).expect("base64 token");
        assert_eq!(
            w.replace(token, "B64"),
            "set -f; IFS=; eval $(printf %s B64 | base64 -d || echo exit 127)"
        );
        // wsl.exe splits its own command line with CommandLineToArgvW rules: lossless.
        let line = win_quote(&a);
        assert_eq!(win_split(&line), a);
        // Also for the other builders.
        let rc = crate::setup::wsl_rc_args(
            "U",
            "/p q",
            &["keys".into(), "add".into(), "it's \"x\" (y)".into()],
        );
        assert_eq!(win_split(&win_quote(&rc)), rc);
        assert!(!rc[7].contains('"') && !rc[7].contains('\''));
    }

    fn shells() -> Vec<&'static str> {
        ["sh", "dash", "bash"]
            .into_iter()
            .filter(|s| have(s))
            .collect()
    }

    /// The decoded NASTY script really runs through the wrapper in every available POSIX
    /// shell (dash is `/bin/sh` on Debian/Ubuntu), keeps stdin and its exit status.
    #[cfg(unix)]
    #[test]
    fn wrapped_script_executes_in_posix_shells() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let script = "echo \"a (b) $(echo hi)\" 'q' ; true && echo `echo x`\nif [ -n \"$HOME\" ]; then\n  echo *.nothing \\\\ done\nfi\ncat <<'EOF'\nheredoc ( ) \"\nEOF\nread l\necho \"stdin=$l\"\nexit 5\n";
        let wrapped = wrap_script(script);
        for sh in shells() {
            let mut child = Command::new(sh)
                .args(["-c", &wrapped])
                .env("HOME", "/tmp")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"from-stdin\n")
                .unwrap();
            let out = child.wait_with_output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert_eq!(
                out.status.code(),
                Some(5),
                "{sh}: {text}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                text, "a (b) hi q\nx\n*.nothing \\ done\nheredoc ( ) \"\nstdin=from-stdin\n",
                "{sh}"
            );
        }
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(
            h.finish_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn picks_the_rc_source() {
        let f = std::env::temp_dir().join(format!("rc-src-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(
            choose_source(Some(f.clone()), "x86_64\n"),
            RcSource::Bundled(f.clone())
        );
        assert_eq!(choose_source(Some(f.clone()), "aarch64"), RcSource::Pages);
        assert_eq!(choose_source(None, "x86_64"), RcSource::Pages);
        assert_eq!(
            choose_source(Some(f.with_extension("missing")), "x86_64"),
            RcSource::Pages
        );
        let _ = std::fs::remove_file(f);
        let st = |v: Option<&str>| RcStatus {
            installed: v.is_some(),
            path: None,
            version: v.map(String::from),
            expected: "1.2.3".into(),
        };
        assert!(st(Some("1.2.3")).is_current());
        assert!(!st(Some("1.2.2")).is_current() && !st(None).is_current());
    }

    #[cfg(unix)]
    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rc-wsl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The stdin-copy script under plain `sh` with HOME in a temp dir.
    #[cfg(unix)]
    #[test]
    fn stdin_copy_script_installs_and_verifies() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        if !have("sha256sum") {
            return;
        }
        let home = tmpdir("copy");
        let bin: Vec<u8> = (0..70_000u32).map(|i| (i * 31 % 251) as u8).collect();
        let run = |sha: &str, data: &[u8]| {
            let mut c = Command::new("sh")
                .args(["-c", &wrap_script(&stdin_copy_script(sha).unwrap())])
                .env("HOME", &home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = c.stdin.take().unwrap();
            let data = data.to_vec();
            let t = std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            });
            let o = c.wait_with_output().unwrap();
            t.join().unwrap();
            (
                o.status.code(),
                String::from_utf8_lossy(&o.stdout).to_string(),
            )
        };
        let (code, log) = run(&sha256_hex(&bin), &bin);
        assert_eq!(code, Some(0), "{log}");
        assert!(
            log.contains("verified") && log.contains("installed"),
            "{log}"
        );
        let rc = home.join(".local/bin/rc");
        assert_eq!(std::fs::read(&rc).unwrap(), bin);
        assert_eq!(
            std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!home.join(".local/bin/rc.tmp").exists());
        // Truncated transfer: rejected, the installed rc stays untouched.
        let (code, log) = run(&sha256_hex(&bin), &bin[..1000]);
        assert_eq!(code, Some(4), "{log}");
        assert!(log.contains("checksum mismatch"));
        assert_eq!(std::fs::read(&rc).unwrap(), bin);
        assert!(!home.join(".local/bin/rc.tmp").exists());
        assert!(stdin_copy_script("nothex").is_err());
        let _ = std::fs::remove_dir_all(home);
    }

    /// Stand-in `wsl.exe` with the real semantics: `-d X [--cd DIR] --exec PROG ARGS...` runs
    /// PROG directly (no shell re-parse); a bare `--` is refused like a bug would be.
    #[cfg(unix)]
    fn write_wsl_stand_in(dir: &std::path::Path, home: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let f = dir.join("wsl.exe");
        std::fs::write(
            &f,
            format!(
                r#"#!/bin/sh
[ "$1" = -d ] || {{ echo "bad args: $*" >&2; exit 90; }}
shift 2
if [ "$1" = --cd ]; then cd "$2" || exit 91; shift 2; fi
[ "$1" = --exec ] || {{ echo "bad args (no --exec): $*" >&2; exit 92; }}
shift
prog=$1; shift
if [ "$1" = -lc ]; then shift; set -- -c "$@"; fi
export HOME={}
exec "$prog" "$@"
"#,
                home.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f
    }

    /// Whole automatic flow through the stand-in: missing rc -> bundled copy -> current;
    /// current rc -> untouched; outdated rc -> replaced; wrong checksum never installs.
    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_rc_installs_the_bundled_binary() {
        use std::os::unix::fs::PermissionsExt;
        if !have("sha256sum") || !have("base64") {
            return;
        }
        let arch = String::from_utf8(
            std::process::Command::new("uname")
                .arg("-m")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        if arch.trim() != "x86_64" {
            return;
        }
        let dir = tmpdir("ensure");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let wsl = write_wsl_stand_in(&dir, &home);
        let prog = wsl.to_str().unwrap();
        let bundled = dir.join("rc-linux-x86_64");
        let body = format!("#!/bin/sh\necho 'rc {}'\n", crate::app_version());
        std::fs::write(&bundled, &body).unwrap();
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let log = |l: String| lines.lock().unwrap().push(l);
        let s = ensure_rc_with(
            prog,
            "Ubuntu-26.04",
            Some(bundled.clone()),
            "https://x.test/s",
            None,
            "0.0.0",
            &log,
        )
        .await
        .unwrap();
        assert!(s.is_current(), "{s:?}");
        assert!(s.path.unwrap().ends_with(".local/bin/rc"));
        let l = lines.lock().unwrap().join("\n");
        assert!(
            l.contains("copying bundled rc") && l.contains("verified") && l.contains("installed"),
            "{l}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".local/bin/rc")).unwrap(),
            body
        );
        // Already current: nothing is copied.
        lines.lock().unwrap().clear();
        let s = ensure_rc_with(
            prog,
            "Ubuntu-26.04",
            Some(bundled.clone()),
            "https://x.test/s",
            None,
            "0.0.0",
            &log,
        )
        .await
        .unwrap();
        assert!(s.is_current() && lines.lock().unwrap().is_empty());
        // Outdated: replaced.
        let rc = home.join(".local/bin/rc");
        std::fs::write(&rc, "#!/bin/sh\necho 'rc 0.0.1'\n").unwrap();
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            check_rc_with(prog, "Ubuntu-26.04")
                .await
                .unwrap()
                .version
                .as_deref(),
            Some("0.0.1")
        );
        let s = ensure_rc_with(
            prog,
            "Ubuntu-26.04",
            Some(bundled.clone()),
            "https://x.test/s",
            None,
            "0.0.0",
            &log,
        )
        .await
        .unwrap();
        assert!(s.is_current());
        // A bundled binary reporting another version is an error, not a silent success.
        std::fs::write(&bundled, "#!/bin/sh\necho 'rc 9.9.9-wrong'\n").unwrap();
        let e = install_bundled_rc_with(
            prog,
            "Ubuntu-26.04",
            Some(bundled),
            "https://x.test/s",
            None,
            "0.0.0",
            &log,
        )
        .await
        .unwrap_err();
        assert!(e.contains("9.9.9-wrong") && e.contains("expected"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
