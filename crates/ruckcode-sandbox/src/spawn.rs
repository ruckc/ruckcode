//! Parent side of the launcher protocol: build the command that re-executes this binary.

#[cfg(unix)]
use crate::launcher::{EXE_FD, POLICY_FD, SUBCOMMAND};
use crate::policy::Policy;
use std::io;
use std::path::PathBuf;

/// Which binary acts as the launcher.
#[derive(Debug, Clone)]
pub enum LauncherExe {
    /// This very binary (`rc`). On Linux it is opened through `/proc/self/exe` and executed
    /// through the descriptor, so replacing the file on disk cannot change what runs.
    SelfExe,
    /// An explicit binary that embeds the launcher (tests).
    Path(PathBuf),
}

#[cfg(unix)]
pub type Fd = std::os::fd::OwnedFd;
#[cfg(not(unix))]
pub type Fd = ();

#[cfg(target_os = "linux")]
fn self_exe_fd() -> io::Result<i32> {
    use std::os::fd::AsRawFd;
    use std::sync::OnceLock;
    static F: OnceLock<Option<std::fs::File>> = OnceLock::new();
    F.get_or_init(|| std::fs::File::open("/proc/self/exe").ok())
        .as_ref()
        .map(|f| f.as_raw_fd())
        .ok_or_else(|| io::Error::other("cannot open /proc/self/exe"))
}

/// A `std` command that runs `argv` under `policy`, plus descriptors to keep open until it has
/// been spawned.
#[cfg(unix)]
pub fn launcher_command(
    exe: &LauncherExe,
    policy: &Policy,
    argv: &[String],
) -> io::Result<(std::process::Command, Vec<Fd>)> {
    use std::io::Write;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    let json = serde_json::to_vec(policy).map_err(io::Error::other)?;
    let mut fds = [0i32; 2];
    #[cfg(target_os = "linux")]
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(not(target_os = "linux"))]
    {
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
    }
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    // A thread feeds the pipe so arbitrarily large policies never block the caller.
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(w);
        let _ = f.write_all(&json);
    });
    #[allow(unused_mut)]
    let mut exe_fd: Option<i32> = None;
    let program: PathBuf = match exe {
        LauncherExe::Path(p) => p.clone(),
        #[cfg(target_os = "linux")]
        LauncherExe::SelfExe => match self_exe_fd() {
            Ok(fd) => {
                exe_fd = Some(fd);
                PathBuf::from(format!("/proc/self/fd/{EXE_FD}"))
            }
            Err(_) => std::env::current_exe()?,
        },
        #[cfg(not(target_os = "linux"))]
        LauncherExe::SelfExe => std::env::current_exe()?,
    };
    let mut cmd = std::process::Command::new(program);
    cmd.arg0("rc");
    cmd.args([SUBCOMMAND, "--policy-fd", &POLICY_FD.to_string(), "--"]);
    cmd.args(argv);
    let rfd = std::os::fd::AsRawFd::as_raw_fd(&r);
    unsafe {
        cmd.pre_exec(move || {
            if rfd != POLICY_FD && libc::dup2(rfd, POLICY_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            if let Some(e) = exe_fd {
                if e != EXE_FD && libc::dup2(e, EXE_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok((cmd, vec![r]))
}

#[cfg(not(unix))]
pub fn launcher_command(
    _: &LauncherExe,
    _: &Policy,
    _: &[String],
) -> io::Result<(std::process::Command, Vec<Fd>)> {
    Err(io::Error::other(
        "sandbox launcher unsupported on this platform",
    ))
}
