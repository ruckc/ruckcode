//! Linux namespace layer (bubblewrap-style, but in-process): user + mount + pid + ipc + uts
//! (+ net) namespaces, a fresh tmpfs root assembled from bind mounts, `pivot_root`, a PID 1
//! reaper, and a loopback forwarder to the host proxy.

use crate::policy::Policy;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn cs(p: &Path) -> CString {
    CString::new(p.as_os_str().as_bytes()).unwrap_or_else(|_| CString::new("/nonexistent").unwrap())
}

fn err(what: impl std::fmt::Display) -> io::Error {
    let e = io::Error::last_os_error();
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

fn mount(
    src: Option<&Path>,
    dst: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> io::Result<()> {
    let s = src.map(cs);
    let d = cs(dst);
    let f = fstype.map(|f| CString::new(f).unwrap());
    let da = data.map(|f| CString::new(f).unwrap());
    let r = unsafe {
        libc::mount(
            s.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            d.as_ptr(),
            f.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            flags,
            da.as_ref()
                .map_or(std::ptr::null(), |c| c.as_ptr() as *const libc::c_void),
        )
    };
    if r != 0 {
        return Err(err(format!("mount {}", dst.display())));
    }
    Ok(())
}

fn rooted(root: &Path, abs: &Path) -> PathBuf {
    root.join(abs.strip_prefix("/").unwrap_or(abs))
}

fn make_mountpoint(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)
    } else {
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dst)
            .map(|_| ())
    }
}

/// Mount flags a read-only remount must preserve (locked inside a user namespace).
fn inherited_flags(src: &Path) -> libc::c_ulong {
    let c = cs(src);
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    (st.f_flag as libc::c_ulong)
        & (libc::MS_NOSUID
            | libc::MS_NODEV
            | libc::MS_NOEXEC
            | libc::MS_NOATIME
            | libc::MS_NODIRATIME
            | libc::MS_RELATIME)
}

fn bind(src: &Path, dst: &Path, ro: bool) -> io::Result<()> {
    make_mountpoint(src, dst)?;
    mount(Some(src), dst, None, libc::MS_BIND | libc::MS_REC, None)?;
    if ro {
        let flags = libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | inherited_flags(src);
        if let Err(e) = mount(None, dst, None, flags, None) {
            // Never leave a writable view of something meant to be read-only.
            unsafe { libc::umount2(cs(dst).as_ptr(), libc::MNT_DETACH) };
            return Err(e);
        }
    }
    Ok(())
}

fn debug(msg: impl std::fmt::Display) {
    if std::env::var_os("RC_SANDBOX_DEBUG").is_some() {
        eprintln!("rc sandbox: {msg}");
    }
}

fn write_file(path: &str, content: &str) -> io::Result<()> {
    std::fs::write(path, content).map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))
}

/// Run `argv` inside the namespaces; returns the exit code for the launcher to exit with.
/// Called from the single-threaded launcher before anything else touches the process.
pub fn run(p: &Policy, argv: &[CString]) -> io::Result<i32> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let mut flags = libc::CLONE_NEWUSER
        | libc::CLONE_NEWNS
        | libc::CLONE_NEWPID
        | libc::CLONE_NEWIPC
        | libc::CLONE_NEWUTS;
    if p.layers.netns {
        flags |= libc::CLONE_NEWNET;
    }
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(err("unshare"));
    }
    write_file("/proc/self/setgroups", "deny")?;
    write_file("/proc/self/uid_map", &format!("{uid} {uid} 1\n"))?;
    write_file("/proc/self/gid_map", &format!("{gid} {gid} 1\n"))?;
    // The first child of the new pid namespace is its PID 1.
    match unsafe { libc::fork() } {
        -1 => Err(err("fork")),
        0 => {
            let code = init_main(p, argv);
            unsafe { libc::_exit(code) }
        }
        pid => Ok(wait_for(pid)),
    }
}

fn wait_for(pid: libc::pid_t) -> i32 {
    loop {
        let mut st = 0;
        let r = unsafe { libc::waitpid(pid, &mut st, 0) };
        if r == -1 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return 126;
        }
        return decode(st);
    }
}

fn decode(st: i32) -> i32 {
    if libc::WIFEXITED(st) {
        libc::WEXITSTATUS(st)
    } else if libc::WIFSIGNALED(st) {
        128 + libc::WTERMSIG(st)
    } else {
        126
    }
}

/// PID 1 of the sandbox: builds the filesystem, starts the forwarder and the command, reaps.
fn init_main(p: &Policy, argv: &[CString]) -> i32 {
    unsafe {
        // Die with the launcher; PID 1 dying takes the whole namespace with it.
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
    }
    if let Err(e) = setup_fs(p) {
        eprintln!("rc sandbox: namespace setup failed: {e}");
        return 126;
    }
    unsafe {
        let h = b"rc-sandbox";
        libc::sethostname(h.as_ptr() as *const libc::c_char, h.len());
    }
    let mut forwarder_needed = false;
    if p.layers.netns {
        if let Err(e) = loopback_up() {
            debug(format!("loopback: {e}"));
        }
        forwarder_needed = p.proxy_port.is_some() && p.proxy_socket.is_some();
    }
    if forwarder_needed {
        if let Err(e) = start_forwarder(p.proxy_port.unwrap()) {
            eprintln!("rc sandbox: network forwarder failed: {e}");
            return 126;
        }
    }
    let cmd_pid = unsafe { libc::fork() };
    if cmd_pid < 0 {
        eprintln!("rc sandbox: fork: {}", io::Error::last_os_error());
        return 126;
    }
    if cmd_pid == 0 {
        unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) };
        crate::launcher::finish_and_exec(p, argv);
    }
    loop {
        let mut st = 0;
        let r = unsafe { libc::waitpid(-1, &mut st, 0) };
        if r == -1 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return 0;
        }
        if r == cmd_pid {
            return decode(st);
        }
    }
}

fn setup_fs(p: &Policy) -> io::Result<()> {
    let stage = p
        .stage_dir
        .as_ref()
        .ok_or_else(|| io::Error::other("namespace mode needs a stage directory"))?;
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;
    let root = stage.join("root");
    std::fs::create_dir_all(&root)?;
    mount(
        None,
        &root,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("mode=0755"),
    )?;
    std::fs::create_dir_all(root.join(".oldroot"))?;

    // /proc for the new pid namespace.
    let proc_dir = root.join("proc");
    std::fs::create_dir_all(&proc_dir)?;
    mount(
        None,
        &proc_dir,
        Some("proc"),
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    )?;
    setup_dev(&root)?;

    // Collect bind mounts, shallow first.
    struct M {
        src: PathBuf,
        dst: PathBuf,
        ro: bool,
    }
    let skip = |q: &Path| q.starts_with("/dev") || q.starts_with("/proc") || q.starts_with("/run");
    let private_tmp = p.private_tmp.as_ref();
    let mut mounts: Vec<M> = Vec::new();
    for r in &p.read_paths {
        if !skip(r) {
            mounts.push(M {
                src: r.clone(),
                dst: r.clone(),
                ro: true,
            });
        }
    }
    for w in &p.write_paths {
        if skip(w) {
            continue;
        }
        if private_tmp.is_some() && (w == Path::new("/tmp") || w == Path::new("/var/tmp")) {
            continue;
        }
        mounts.push(M {
            src: w.clone(),
            dst: w.clone(),
            ro: false,
        });
    }
    if let Some(t) = private_tmp {
        mounts.push(M {
            src: t.join("tmp"),
            dst: "/tmp".into(),
            ro: false,
        });
        mounts.push(M {
            src: t.join("vartmp"),
            dst: "/var/tmp".into(),
            ro: false,
        });
    }
    mounts.sort_by_key(|m| m.dst.components().count());
    // A read-only mount below a read-only ancestor adds nothing (the bind is recursive).
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    for m in &mounts {
        if !m.src.exists() {
            continue;
        }
        if done
            .iter()
            .any(|(d, ro)| *ro && m.ro && m.dst.starts_with(d))
        {
            continue;
        }
        match bind(&m.src, &rooted(&root, &m.dst), m.ro) {
            Ok(()) => done.push((m.dst.clone(), m.ro)),
            Err(e) if m.dst == p.project_root => return Err(e),
            Err(e) => debug(format!("skipping {}: {e}", m.dst.display())),
        }
    }
    for (link, target) in &p.symlinks {
        let l = rooted(&root, link);
        if let Some(par) = l.parent() {
            std::fs::create_dir_all(par)?;
        }
        let _ = std::os::unix::fs::symlink(target, &l);
    }
    if let Some(sock) = &p.proxy_socket {
        let run = root.join("run");
        std::fs::create_dir_all(&run)?;
        if let Err(e) = bind(sock, &run.join("rc-proxy.sock"), false) {
            debug(format!("proxy socket: {e}"));
        }
    }
    // Masks and protected config, last so they cover whatever was bound before.
    for m in &p.mask_paths {
        let dst = rooted(&root, m);
        if !dst.exists() {
            continue;
        }
        let r = if m.is_dir() {
            mount(
                None,
                &dst,
                Some("tmpfs"),
                libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                Some("mode=000,size=4k"),
            )
        } else {
            mount(
                Some(Path::new("/dev/null")),
                &dst,
                None,
                libc::MS_BIND,
                None,
            )
        };
        if let Err(e) = r {
            // A mask that fails must not fail open: refuse to start.
            return Err(io::Error::other(format!(
                "cannot mask {}: {e}",
                m.display()
            )));
        }
    }
    for q in &p.protected_ro {
        let dst = rooted(&root, q);
        if dst.exists() {
            if let Err(e) = bind(q, &dst, true) {
                return Err(io::Error::other(format!(
                    "cannot protect {}: {e}",
                    q.display()
                )));
            }
        }
    }

    // pivot_root into the new tree and drop the old one.
    let old = root.join(".oldroot");
    let (r, o) = (cs(&root), cs(&old));
    if unsafe { libc::syscall(libc::SYS_pivot_root, r.as_ptr(), o.as_ptr()) } != 0 {
        return Err(err("pivot_root"));
    }
    std::env::set_current_dir("/")?;
    unsafe {
        let old = CString::new("/.oldroot").unwrap();
        libc::umount2(old.as_ptr(), libc::MNT_DETACH);
    }
    let _ = std::fs::remove_dir("/.oldroot");
    let _ = mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    );
    Ok(())
}

/// A minimal /dev: null, zero, full, random, urandom, tty, pts, shm and the fd symlinks.
fn setup_dev(root: &Path) -> io::Result<()> {
    let dev = root.join("dev");
    std::fs::create_dir_all(&dev)?;
    mount(
        None,
        &dev,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some("mode=0755,size=64k"),
    )?;
    for n in ["null", "zero", "full", "random", "urandom", "tty"] {
        let host = Path::new("/dev").join(n);
        if host.exists() {
            if let Err(e) = bind(&host, &dev.join(n), false) {
                debug(format!("/dev/{n}: {e}"));
            }
        }
    }
    let pts = dev.join("pts");
    std::fs::create_dir_all(&pts)?;
    if mount(
        None,
        &pts,
        Some("devpts"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some("newinstance,ptmxmode=0666,mode=0620"),
    )
    .is_ok()
    {
        let _ = std::os::unix::fs::symlink("pts/ptmx", dev.join("ptmx"));
    }
    let shm = dev.join("shm");
    std::fs::create_dir_all(&shm)?;
    let _ = mount(
        None,
        &shm,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("mode=1777,size=64m"),
    );
    for (l, t) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        let _ = std::os::unix::fs::symlink(t, dev.join(l));
    }
    Ok(())
}

fn loopback_up() -> io::Result<()> {
    unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if s < 0 {
            return Err(err("socket"));
        }
        let mut ifr: libc::ifreq = std::mem::zeroed();
        for (i, b) in b"lo\0".iter().enumerate() {
            ifr.ifr_name[i] = *b as libc::c_char;
        }
        ifr.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        let r = libc::ioctl(s, libc::SIOCSIFFLAGS as _, &ifr);
        libc::close(s);
        if r != 0 {
            return Err(err("SIOCSIFFLAGS lo"));
        }
    }
    Ok(())
}

/// Fork the in-namespace forwarder: 127.0.0.1:port -> the host proxy's unix socket. Returns once
/// the forwarder is listening.
fn start_forwarder(port: u16) -> io::Result<()> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(err("pipe"));
    }
    match unsafe { libc::fork() } {
        -1 => Err(err("fork")),
        0 => {
            unsafe {
                libc::close(fds[0]);
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
            }
            forward_loop(port, fds[1])
        }
        _ => {
            unsafe { libc::close(fds[1]) };
            let mut b = [0u8; 1];
            let n = unsafe { libc::read(fds[0], b.as_mut_ptr() as *mut _, 1) };
            unsafe { libc::close(fds[0]) };
            if n == 1 {
                Ok(())
            } else {
                Err(io::Error::other("forwarder did not start"))
            }
        }
    }
}

fn forward_loop(port: u16, ready: i32) -> ! {
    use std::net::{Shutdown, TcpListener};
    use std::os::unix::net::UnixStream;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) => unsafe { libc::_exit(1) },
    };
    unsafe {
        libc::write(ready, b"1".as_ptr() as *const _, 1);
        libc::close(ready);
    }
    for conn in listener.incoming() {
        let Ok(tcp) = conn else { continue };
        std::thread::spawn(move || {
            let Ok(unix) = UnixStream::connect("/run/rc-proxy.sock") else {
                return;
            };
            let (mut t2, mut u2) = match (tcp.try_clone(), unix.try_clone()) {
                (Ok(a), Ok(b)) => (a, b),
                _ => return,
            };
            let (mut t1, mut u1) = (tcp, unix);
            let up = std::thread::spawn(move || {
                let _ = std::io::copy(&mut t2, &mut u2);
                let _ = u2.shutdown(Shutdown::Write);
            });
            let _ = std::io::copy(&mut u1, &mut t1);
            let _ = t1.shutdown(Shutdown::Write);
            let _ = up.join();
        });
    }
    unsafe { libc::_exit(0) }
}
