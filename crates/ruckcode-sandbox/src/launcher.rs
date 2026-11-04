//! The self re-exec launcher: `rc __sandbox-exec --policy-fd N -- program args...`.
//!
//! It runs before anything else in `main` (single-threaded), reads the JSON policy from an
//! inherited descriptor, restricts itself and then `exec`s the command, so every descendant
//! inherits the restrictions. The policy never travels through argv or the environment.

#[cfg(unix)]
use crate::policy::Policy;
#[cfg(unix)]
use std::ffi::CString;
use std::ffi::OsString;

/// Descriptor numbers the parent installs in the child (see `runner`).
pub const POLICY_FD: i32 = 198;
pub const EXE_FD: i32 = 199;
/// First argument that selects launcher mode.
pub const SUBCOMMAND: &str = "__sandbox-exec";

/// If this process was started as the launcher, run it and never return.
pub fn maybe_run() {
    let mut args = std::env::args_os();
    args.next();
    if args.next().is_some_and(|a| a == SUBCOMMAND) {
        run_from_args(args.collect());
    }
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("rc sandbox: {msg}");
    std::process::exit(126)
}

/// `args` are everything after the subcommand name.
pub fn run_from_args(args: Vec<OsString>) -> ! {
    #[cfg(not(unix))]
    {
        let _ = args;
        die("the sandbox launcher is not available on this platform");
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::io::FromRawFd;
        let mut fd = POLICY_FD;
        let mut it = args.into_iter();
        let mut cmd: Vec<CString> = Vec::new();
        while let Some(a) = it.next() {
            if a == "--policy-fd" {
                fd = it
                    .next()
                    .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                    .unwrap_or_else(|| die("bad --policy-fd"));
            } else if a == "--" {
                cmd = it
                    .by_ref()
                    .map(|a| CString::new(a.into_vec()).unwrap_or_else(|_| die("NUL in argument")))
                    .collect();
                break;
            } else {
                die(format!("unexpected argument {a:?}"));
            }
        }
        if cmd.is_empty() {
            die("no command given");
        }
        let mut text = String::new();
        let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
        if let Err(e) = f.read_to_string(&mut text) {
            die(format!("cannot read policy: {e}"));
        }
        drop(f);
        unsafe {
            libc::close(EXE_FD);
        }
        let policy: Policy =
            serde_json::from_str(&text).unwrap_or_else(|e| die(format!("bad policy: {e}")));
        if policy.version != Policy::VERSION {
            die("policy version mismatch");
        }
        launch(&policy, &cmd)
    }
}

#[cfg(unix)]
fn launch(p: &Policy, argv: &[CString]) -> ! {
    #[cfg(target_os = "linux")]
    if p.layers.namespaces {
        match crate::ns::run(p, argv) {
            Ok(code) => std::process::exit(code),
            Err(e) => die(format!("namespace setup failed: {e}")),
        }
    }
    finish_and_exec(p, argv)
}

/// Apply the in-process restrictions (Landlock, seccomp, Seatbelt) and exec the command.
#[cfg(unix)]
pub fn finish_and_exec(p: &Policy, argv: &[CString]) -> ! {
    if let Err(e) = std::env::set_current_dir(&p.cwd) {
        die(format!("cannot enter {}: {e}", p.cwd.display()));
    }
    #[cfg(target_os = "linux")]
    if let Err(e) = crate::linux::apply_restrictions(p) {
        die(e);
    }
    #[cfg(target_os = "macos")]
    if p.layers.seatbelt {
        if let Err(e) = crate::macos::apply(p) {
            die(e);
        }
    }
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe {
        libc::execvp(argv[0].as_ptr(), ptrs.as_ptr());
    }
    eprintln!(
        "rc sandbox: cannot execute {}: {}",
        argv[0].to_string_lossy(),
        std::io::Error::last_os_error()
    );
    unsafe { libc::_exit(127) }
}
