//! Capability detection, the enforcement plan derived from it, and the status report.

use crate::config::{NamespaceMode, NetworkMode, SandboxConfig};
use crate::policy::Layers;
use crate::spawn::LauncherExe;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub platform: String,
    pub landlock_abi: Option<u32>,
    pub seccomp: bool,
    pub namespaces: bool,
    /// Why namespaces are unavailable (with a remedy where one is known).
    pub namespaces_reason: Option<String>,
    pub seatbelt: bool,
    /// The launcher binary could not be run at all (it does not embed the launcher).
    #[serde(default)]
    pub launcher_error: Option<String>,
}

impl Capabilities {
    pub fn landlock_net(&self) -> bool {
        self.landlock_abi.is_some_and(|a| a >= 4)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub layers: Layers,
    pub warnings: Vec<String>,
    /// Set when the commands must not run (strict mode, or a required layer is missing).
    pub refusal: Option<String>,
    /// True when no layer at all applies (native Windows, unsupported platforms).
    pub unsandboxed: bool,
}

pub const WINDOWS_NOTICE: &str =
    "sandbox unavailable on native Windows; commands run unsandboxed — use WSL";

/// Work out which layers will run for `cfg` on a system with `caps`.
pub fn plan(caps: &Capabilities, cfg: &SandboxConfig) -> Plan {
    let mut p = Plan::default();
    let mut weak: Vec<String> = Vec::new();
    if let Some(e) = &caps.launcher_error {
        // Fail closed: without the launcher nothing can be confined.
        p.refusal = Some(format!(
            "the sandbox launcher cannot run ({e}); the binary must handle `__sandbox-exec`"
        ));
        return p;
    }
    match caps.platform.as_str() {
        "linux" => {
            let ns = cfg.namespaces != NamespaceMode::Off && caps.namespaces;
            if cfg.namespaces == NamespaceMode::Required && !ns {
                p.refusal = Some(format!(
                    "namespaces are required (sandbox.namespaces = \"required\") but unavailable: {}",
                    caps.namespaces_reason.as_deref().unwrap_or("unknown reason")
                ));
            }
            let landlock = caps.landlock_abi.is_some();
            let netns = ns && cfg.network != NetworkMode::Full;
            // A network namespace already isolates the network; Landlock TCP rules would only break
            // servers the command starts inside it.
            let landlock_net =
                landlock && caps.landlock_net() && cfg.network != NetworkMode::Full && !netns;
            p.layers = Layers {
                landlock,
                landlock_net,
                seccomp: caps.seccomp,
                seccomp_net: cfg.network != NetworkMode::Full,
                namespaces: ns,
                netns,
                seatbelt: false,
            };
            if !landlock && !ns {
                weak.push(
                    "filesystem confinement unavailable: this kernel has no Landlock (needs Linux 5.13+ with the landlock LSM enabled) and namespaces are unavailable".into(),
                );
            }
            if !caps.seccomp {
                weak.push("seccomp is unavailable: dangerous syscalls and UDP/raw sockets are not blocked".into());
            }
            if cfg.network != NetworkMode::Full && !(netns || landlock_net) {
                weak.push(format!(
                    "network isolation is not enforced (needs Landlock ABI 4 / Linux 6.7+, or user namespaces): commands could bypass the {} proxy policy with direct TCP",
                    if cfg.network == NetworkMode::Off { "off" } else { "allowlist" }
                ));
            }
            if cfg.namespaces != NamespaceMode::Off && !caps.namespaces {
                p.warnings.push(format!(
                    "namespaces unavailable ({}): using Landlock + seccomp only; project .env files are not masked and ruckcode's project config is only protected by restore-after-run",
                    caps.namespaces_reason.as_deref().unwrap_or("unknown reason")
                ));
            }
        }
        "macos" => {
            p.layers = Layers {
                seatbelt: caps.seatbelt,
                ..Default::default()
            };
            if !caps.seatbelt {
                weak.push("Seatbelt (sandbox_init) is unavailable".into());
            }
        }
        _ => {
            p.unsandboxed = true;
            weak.push(if caps.platform == "windows" {
                WINDOWS_NOTICE.to_string()
            } else {
                format!("sandboxing is not supported on {}", caps.platform)
            });
        }
    }
    if cfg.strict && !weak.is_empty() && p.refusal.is_none() {
        p.refusal = Some(format!(
            "sandbox.strict is set and full enforcement is unavailable: {}",
            weak.join("; ")
        ));
    }
    p.warnings.extend(weak);
    p
}

#[cfg(target_os = "linux")]
fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Explain why unprivileged namespaces failed, with a remedy where we know one.
#[cfg(target_os = "linux")]
fn explain_userns_failure(raw: &str) -> String {
    if read_trim("/proc/sys/user/max_user_namespaces").as_deref() == Some("0") {
        return "user namespaces are disabled (user.max_user_namespaces=0)".into();
    }
    if read_trim("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        return "unprivileged user namespaces are disabled (kernel.unprivileged_userns_clone=0)"
            .into();
    }
    if read_trim("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") {
        return "AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns=1); grant `userns` to the rc binary with an AppArmor profile (see docs/SANDBOX.md)".into();
    }
    let in_container = std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("docker") || c.contains("kubepods") || c.contains("containerd"))
            .unwrap_or(false);
    if in_container {
        return format!("running inside a container whose seccomp/mount policy blocks namespaces ({raw}); docker's default profile blocks unshare");
    }
    raw.to_string()
}

#[cfg(target_os = "linux")]
fn probe_namespaces(exe: &LauncherExe) -> Result<(), String> {
    use crate::policy::{build_policy, BuildInput};
    let base = std::env::temp_dir().join(format!("rc-sbx-probe-{}", std::process::id()));
    let stage = base.join("stage");
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(stage.join("tmp")).ok();
    std::fs::create_dir_all(stage.join("vartmp")).ok();
    let cfg = SandboxConfig {
        network: NetworkMode::Off,
        ..Default::default()
    };
    let policy = build_policy(&BuildInput {
        cfg: &cfg,
        cwd: &proj,
        project_root: &proj,
        home: None,
        config_dir: None,
        layers: Layers {
            landlock: false,
            landlock_net: false,
            seccomp: false,
            seccomp_net: false,
            namespaces: true,
            netns: true,
            seatbelt: false,
        },
        proxy_port: None,
        proxy_socket: None,
        stage_dir: Some(stage.clone()),
        private_tmp: Some(stage.clone()),
    });
    let truebin = ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("true");
    let result = (|| {
        let (mut cmd, fds) = crate::spawn::launcher_command(exe, &policy, &[truebin.to_string()])
            .map_err(|e| e.to_string())?;
        let out = cmd
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string());
        drop(fds);
        let out = out?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    })();
    let _ = std::fs::remove_dir_all(&base);
    result
}

/// Run a command through the launcher with no layers enabled, to prove the launcher exists.
#[cfg(unix)]
fn launcher_check(exe: &LauncherExe) -> Result<(), String> {
    use crate::policy::{build_policy, BuildInput};
    let cfg = SandboxConfig::default();
    let dir = std::env::temp_dir();
    let policy = build_policy(&BuildInput {
        cfg: &cfg,
        cwd: &dir,
        project_root: &dir,
        home: None,
        config_dir: None,
        layers: Layers::default(),
        proxy_port: None,
        proxy_socket: None,
        stage_dir: None,
        private_tmp: None,
    });
    let truebin = ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("true");
    let (mut cmd, fds) = crate::spawn::launcher_command(exe, &policy, &[truebin.to_string()])
        .map_err(|e| e.to_string())?;
    let out = cmd
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string());
    drop(fds);
    let out = out?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Probe the running system. Runs the real launcher to see whether it works and whether
/// namespaces are usable.
pub fn detect(exe: &LauncherExe) -> Capabilities {
    #[allow(unused_mut)]
    let mut c = Capabilities {
        platform: std::env::consts::OS.to_string(),
        ..Default::default()
    };
    #[cfg(unix)]
    if let Err(e) = launcher_check(exe) {
        c.launcher_error = Some(e);
        return c;
    }
    #[cfg(target_os = "linux")]
    {
        c.landlock_abi = crate::linux::landlock_abi();
        c.seccomp = crate::linux::seccomp_available();
        match probe_namespaces(exe) {
            Ok(()) => c.namespaces = true,
            Err(raw) => {
                let raw = raw.trim_start_matches("rc sandbox: ").to_string();
                c.namespaces_reason = Some(explain_userns_failure(&raw));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = exe;
        c.seatbelt = true;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = exe;
    c
}

/// Human-readable status for `rc sandbox status`.
pub fn report(caps: &Capabilities, cfg: &SandboxConfig) -> String {
    use std::fmt::Write;
    let plan = plan(caps, cfg);
    let mut s = String::new();
    let yn = |b: bool| if b { "yes" } else { "no" };
    let _ = writeln!(s, "platform: {}", caps.platform);
    let _ = writeln!(s, "enabled: {}  strict: {}", cfg.enabled, cfg.strict);
    if let Some(e) = &caps.launcher_error {
        let _ = writeln!(s, "launcher: BROKEN ({e})");
    }
    match caps.platform.as_str() {
        "linux" => {
            match caps.landlock_abi {
                Some(a) => {
                    let _ = writeln!(s, "landlock: yes (ABI v{a}; network rules: {})", yn(a >= 4));
                }
                None => {
                    let _ = writeln!(s, "landlock: no");
                }
            }
            let _ = writeln!(s, "seccomp: {}", yn(caps.seccomp));
            let _ = writeln!(
                s,
                "user namespaces: {}{}",
                yn(caps.namespaces),
                caps.namespaces_reason
                    .as_ref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
        }
        "macos" => {
            let _ = writeln!(s, "seatbelt: {}", yn(caps.seatbelt));
        }
        _ => {
            let _ = writeln!(s, "{}", WINDOWS_NOTICE);
        }
    }
    let l = plan.layers;
    let mut on: Vec<&str> = Vec::new();
    for (flag, name) in [
        (l.namespaces, "namespaces (user/mount/pid/ipc/uts)"),
        (l.netns, "network namespace + proxy forwarder"),
        (l.landlock, "landlock filesystem"),
        (l.landlock_net, "landlock network"),
        (l.seccomp, "seccomp"),
        (l.seatbelt, "seatbelt"),
    ] {
        if flag {
            on.push(name);
        }
    }
    let _ = writeln!(
        s,
        "active layers: {}",
        if on.is_empty() {
            "none".to_string()
        } else {
            on.join(", ")
        }
    );
    let _ = writeln!(
        s,
        "policy: network={:?} namespaces={:?} allowed_domains={} extra_write={} extra_read={} deny_read={}",
        cfg.network,
        cfg.namespaces,
        cfg.allowed_domains.len(),
        cfg.extra_write_paths.len(),
        cfg.extra_read_paths.len(),
        cfg.deny_read_paths.len()
    );
    for w in &plan.warnings {
        let _ = writeln!(s, "warning: {w}");
    }
    if let Some(r) = &plan.refusal {
        let _ = writeln!(s, "REFUSING TO RUN COMMANDS: {r}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(ns: bool, abi: Option<u32>, seccomp: bool) -> Capabilities {
        Capabilities {
            platform: "linux".into(),
            landlock_abi: abi,
            seccomp,
            namespaces: ns,
            namespaces_reason: (!ns).then(|| "restricted".into()),
            seatbelt: false,
            launcher_error: None,
        }
    }

    #[test]
    fn full_linux_uses_everything() {
        let p = plan(&linux(true, Some(4), true), &SandboxConfig::default());
        assert!(
            p.layers.namespaces && p.layers.netns && p.layers.landlock && !p.layers.landlock_net
        );
        assert!(p.refusal.is_none() && p.warnings.is_empty());
    }

    #[test]
    fn ubuntu_2404_default_falls_back_to_landlock() {
        let p = plan(&linux(false, Some(4), true), &SandboxConfig::default());
        assert!(!p.layers.namespaces && p.layers.landlock_net);
        assert!(p.refusal.is_none());
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("namespaces unavailable")));
    }

    #[test]
    fn old_kernel_degraded_and_strict_refuses() {
        let caps = linux(false, Some(3), true);
        let p = plan(&caps, &SandboxConfig::default());
        assert!(p.refusal.is_none());
        assert!(p.warnings.iter().any(|w| w.contains("network isolation")));
        let strict = SandboxConfig {
            strict: true,
            ..Default::default()
        };
        assert!(plan(&caps, &strict).refusal.is_some());
        // Network off is still not enforced without netns/landlock net.
        let full = SandboxConfig {
            network: NetworkMode::Full,
            strict: true,
            ..Default::default()
        };
        assert!(plan(&caps, &full).refusal.is_none());
    }

    #[test]
    fn required_namespaces_refuse() {
        let cfg = SandboxConfig {
            namespaces: NamespaceMode::Required,
            ..Default::default()
        };
        let p = plan(&linux(false, Some(4), true), &cfg);
        assert!(p.refusal.unwrap().contains("restricted"));
        let off = SandboxConfig {
            namespaces: NamespaceMode::Off,
            ..Default::default()
        };
        let p = plan(&linux(true, Some(4), true), &off);
        assert!(!p.layers.namespaces && p.warnings.is_empty());
    }

    #[test]
    fn windows_runs_unsandboxed_unless_strict() {
        let caps = Capabilities {
            platform: "windows".into(),
            ..Default::default()
        };
        let p = plan(&caps, &SandboxConfig::default());
        assert!(p.unsandboxed && p.refusal.is_none());
        assert!(p.warnings[0].contains("use WSL"));
        let strict = SandboxConfig {
            strict: true,
            ..Default::default()
        };
        assert!(plan(&caps, &strict).refusal.is_some());
    }

    #[test]
    fn report_mentions_layers() {
        let r = report(&linux(true, Some(4), true), &SandboxConfig::default());
        assert!(r.contains("landlock: yes (ABI v4") && r.contains("active layers:"));
    }
}
