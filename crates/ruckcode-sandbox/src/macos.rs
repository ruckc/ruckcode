//! macOS: Seatbelt through the C API in libSystem (no `sandbox-exec` binary). The SBPL profile
//! is generated here from the same [`Policy`]; generation is pure so it is unit-tested everywhere.

use crate::config::NetworkMode;
use crate::policy::Policy;
use std::path::Path;

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn sub(p: &Path) -> String {
    format!("(subpath {})", q(&p.to_string_lossy()))
}

fn lit(p: &Path) -> String {
    format!("(literal {})", q(&p.to_string_lossy()))
}

/// Generate the profile. Rules never depend on evaluation order: every restriction is a deny
/// with `require-not` exceptions on top of `(allow default)`.
pub fn profile(p: &Policy) -> String {
    let mut s = String::from("(version 1)\n(allow default)\n");
    // Writes: only the allowed roots, devices and temp.
    let mut keep: Vec<String> = p.write_paths.iter().map(|w| path_filter_any(w)).collect();
    for dev in [
        "/dev/null",
        "/dev/zero",
        "/dev/dtracehelper",
        "/dev/tty",
        "/dev/random",
        "/dev/urandom",
    ] {
        keep.push(lit(Path::new(dev)));
    }
    keep.push("(regex #\"^/dev/(tty|pty)[a-z0-9]*$\")".into());
    keep.push("(regex #\"^/dev/fd/[0-9]+$\")".into());
    let mut conds: Vec<String> = keep.iter().map(|k| format!("(require-not {k})")).collect();
    if conds.is_empty() {
        conds.push("(require-not (literal \"/\"))".into());
    }
    s.push_str(&format!(
        "(deny file-write* (require-all {}))\n",
        conds.join(" ")
    ));
    // ruckcode's own config stays read-only even inside the writable project.
    for r in &p.protected_ro {
        s.push_str(&format!("(deny file-write* {})\n", path_filter_any(r)));
    }
    // Secrets: unreadable and unwritable.
    for d in p.deny_read.iter().chain(p.mask_paths.iter()) {
        s.push_str(&format!(
            "(deny file-read* file-write* {})\n",
            path_filter_any(d)
        ));
    }
    // Network.
    match p.network {
        NetworkMode::Full => {}
        NetworkMode::Off => {
            s.push_str("(deny network-bind network-inbound)\n");
            s.push_str("(deny network-outbound (require-not (remote unix-socket)))\n");
        }
        NetworkMode::Allowlist => {
            s.push_str("(deny network-bind network-inbound)\n");
            // Deny every outbound connection, then re-allow unix sockets and the proxy. SBPL
            // lets a later rule win, so the allows must come after the deny. The host part of a
            // network filter can only be `*` or `localhost` (a literal address is a profile
            // error), so the proxy is named by port on `localhost`, which the kernel matches
            // against the loopback interface; the proxy listens on 127.0.0.1 and HTTP_PROXY
            // names 127.0.0.1 so the connection is always to a loopback address.
            s.push_str("(deny network-outbound)\n");
            s.push_str("(allow network-outbound (remote unix-socket))\n");
            for port in &p.connect_ports {
                s.push_str(&format!(
                    "(allow network-outbound (remote ip \"localhost:{port}\"))\n"
                ));
            }
        }
    }
    s
}

/// Deny rules apply whether or not the path exists yet, so existence can't pick the filter.
fn path_filter_any(p: &Path) -> String {
    if p.is_file() {
        lit(p)
    } else {
        sub(p)
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn sandbox_init_with_parameters(
        profile: *const libc::c_char,
        flags: u64,
        parameters: *const *const libc::c_char,
        errorbuf: *mut *mut libc::c_char,
    ) -> libc::c_int;
    fn sandbox_free_error(errorbuf: *mut libc::c_char);
}

/// Apply the profile to the current process (irreversible; inherited by children).
#[cfg(target_os = "macos")]
pub fn apply(p: &Policy) -> Result<(), String> {
    let prof = std::ffi::CString::new(profile(p)).map_err(|e| e.to_string())?;
    let params: [*const libc::c_char; 1] = [std::ptr::null()];
    let mut err: *mut libc::c_char = std::ptr::null_mut();
    let r = unsafe { sandbox_init_with_parameters(prof.as_ptr(), 0, params.as_ptr(), &mut err) };
    if r != 0 {
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            let m = unsafe { std::ffi::CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
            unsafe { sandbox_free_error(err) };
            m
        };
        return Err(format!("seatbelt: {msg}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Layers;

    fn pol(network: NetworkMode) -> Policy {
        Policy {
            version: Policy::VERSION,
            cwd: "/work".into(),
            project_root: "/work".into(),
            read_paths: vec![],
            write_paths: vec!["/work".into(), "/private/tmp".into()],
            symlinks: vec![],
            mask_paths: vec!["/work/.env".into()],
            protected_ro: vec!["/work/.ruckcode".into()],
            deny_read: vec!["/Users/u/.ssh".into()],
            network,
            proxy_port: Some(5555),
            connect_ports: vec![5555],
            proxy_socket: None,
            stage_dir: None,
            private_tmp: None,
            layers: Layers {
                seatbelt: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn profile_allowlist() {
        let s = profile(&pol(NetworkMode::Allowlist));
        assert!(s.starts_with("(version 1)\n(allow default)"));
        assert!(s.contains("(require-not (subpath \"/work\"))"));
        assert!(s.contains("(require-not (subpath \"/private/tmp\"))"));
        assert!(s.contains("(deny file-write* (subpath \"/work/.ruckcode\"))"));
        assert!(s.contains("(deny file-read* file-write* (subpath \"/Users/u/.ssh\"))"));
        assert!(s.contains("(allow network-outbound (remote ip \"localhost:5555\"))"));
        // The allows must follow the blanket deny (later rules win in SBPL).
        let deny = s.find("(deny network-outbound)").unwrap();
        assert!(s.find("(allow network-outbound (remote ip").unwrap() > deny);
        assert!(
            s.find("(allow network-outbound (remote unix-socket))")
                .unwrap()
                > deny
        );
        // Only the proxy port is opened, never a wildcard host.
        assert!(!s.contains("\"*:"));
        assert_eq!(s.matches("remote ip").count(), 1);
        assert!(s.contains("(deny network-bind network-inbound)"));
    }

    #[test]
    fn profile_network_modes() {
        let off = profile(&pol(NetworkMode::Off));
        assert!(off.contains("(deny network-outbound (require-not (remote unix-socket)))"));
        assert!(!off.contains("localhost:"));
        let full = profile(&pol(NetworkMode::Full));
        assert!(!full.contains("network"));
    }

    #[test]
    fn quoting() {
        assert_eq!(q("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
