//! Linux enforcement: Landlock (filesystem + TCP), seccomp and privilege dropping.

use crate::policy::Policy;
use std::collections::BTreeMap;

/// The Landlock ABI version the running kernel supports, if Landlock is enabled.
pub fn landlock_abi() -> Option<u32> {
    // landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)
    let r = unsafe { libc::syscall(444, std::ptr::null::<u8>(), 0usize, 1u32) };
    (r > 0).then_some(r as u32)
}

/// Whether seccomp filters can be installed.
pub fn seccomp_available() -> bool {
    // PR_GET_SECCOMP returns 0 (off) / 2 (filter) when supported, EINVAL otherwise.
    unsafe { libc::prctl(libc::PR_GET_SECCOMP) >= 0 }
}

fn nnp() -> Result<(), String> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

pub fn apply_landlock(p: &Policy) -> Result<(), String> {
    use landlock::{
        path_beneath_rules, Access, AccessFs, AccessNet, NetPort, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, ABI,
    };
    let abi = ABI::V4;
    let e = |what: &str, e: landlock::RulesetError| format!("landlock {what}: {e}");
    let mut rs = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|x| e("fs access", x))?;
    if p.layers.landlock_net {
        rs = rs
            .handle_access(AccessNet::from_all(abi))
            .map_err(|x| e("net access", x))?;
    }
    let mut rs = rs.create().map_err(|x| e("create", x))?;
    rs = rs
        .add_rules(path_beneath_rules(&p.read_paths, AccessFs::from_read(abi)))
        .map_err(|x| e("read rules", x))?;
    rs = rs
        .add_rules(path_beneath_rules(&p.write_paths, AccessFs::from_all(abi)))
        .map_err(|x| e("write rules", x))?;
    if p.layers.landlock_net {
        for port in &p.connect_ports {
            rs = rs
                .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
                .map_err(|x| e("net rule", x))?;
        }
    }
    let status = rs.restrict_self().map_err(|x| e("restrict", x))?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err("landlock is not enforced by this kernel".into());
    }
    Ok(())
}

/// Syscalls denied outright (EPERM).
fn denied_syscalls() -> Vec<i64> {
    vec![
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_userfaultfd,
        libc::SYS_open_by_handle_at,
        libc::SYS_move_mount,
        libc::SYS_open_tree,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_mount_setattr,
    ]
}

pub fn apply_seccomp(net_filter: bool) -> Result<(), String> {
    use seccompiler::{
        apply_filter, BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op,
        SeccompCondition as Cond, SeccompFilter, SeccompRule,
    };
    let se = |e: seccompiler::Error| format!("seccomp: {e}");
    let be = |e: seccompiler::BackendError| format!("seccomp: {e}");
    let cond = |arg: u8, op: Op, v: u64| Cond::new(arg, Len::Dword, op, v).map_err(be);
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for s in denied_syscalls() {
        rules.insert(s, vec![]);
    }
    // personality(): allow only the query form.
    rules.insert(
        libc::SYS_personality,
        vec![SeccompRule::new(vec![cond(0, Op::Ne, 0xffff_ffff)?]).map_err(be)?],
    );
    let mut sock = vec![
        // AF_PACKET
        SeccompRule::new(vec![cond(0, Op::Eq, libc::AF_PACKET as u64)?]).map_err(be)?,
        // AF_NETLINK other than NETLINK_ROUTE (0)
        SeccompRule::new(vec![
            cond(0, Op::Eq, libc::AF_NETLINK as u64)?,
            cond(2, Op::Ne, 0)?,
        ])
        .map_err(be)?,
    ];
    if net_filter {
        for fam in [libc::AF_INET, libc::AF_INET6] {
            for ty in [libc::SOCK_DGRAM, libc::SOCK_RAW] {
                sock.push(
                    SeccompRule::new(vec![
                        cond(0, Op::Eq, fam as u64)?,
                        cond(1, Op::MaskedEq(0xf), ty as u64)?,
                    ])
                    .map_err(be)?,
                );
            }
        }
    }
    rules.insert(libc::SYS_socket, sock);
    let arch = std::env::consts::ARCH
        .try_into()
        .map_err(|e: seccompiler::BackendError| format!("seccomp arch: {e}"))?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(be)?;
    let prog: BpfProgram = filter.try_into().map_err(be)?;
    nnp()?;
    apply_filter(&prog).map_err(se)
}

/// Drop every capability (relevant inside a user namespace and when running as root).
pub fn drop_caps() {
    unsafe {
        for cap in 0..64 {
            libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0);
        }
        #[repr(C)]
        struct Hdr {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        let hdr = Hdr {
            version: 0x2008_0522,
            pid: 0,
        };
        let data = [Data {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        libc::syscall(libc::SYS_capset, &hdr as *const Hdr, data.as_ptr());
    }
}

/// Apply every enabled restriction to the current (single-threaded) process.
pub fn apply_restrictions(p: &Policy) -> Result<(), String> {
    nnp()?;
    if p.layers.landlock {
        apply_landlock(p)?;
    }
    if p.layers.seccomp {
        apply_seccomp(p.layers.seccomp_net)?;
    }
    drop_caps();
    Ok(())
}
