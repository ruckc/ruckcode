# Command sandbox

`rc` confines the processes the agent starts (the `Bash` tool and mise task tools) using only
mechanisms built into its own binary: no `bwrap`, no `sandbox-exec`, nothing on `PATH` that could be
swapped. The model cannot ask to run outside the sandbox. Only the user can change the policy
(`config.toml`, `modes.toml`).

Hooks and MCP servers are configured by the user and run as before; they are not sandboxed.

## How it works

1. The tool re-executes the running binary as a hidden launcher:
   `rc __sandbox-exec --policy-fd N -- sh -c <cmd>`. On Linux the binary is opened through
   `/proc/self/exe` and executed through that descriptor, so replacing the file on disk cannot change
   what runs. The desktop app embeds the same launcher (its `main` handles `__sandbox-exec`).
2. The JSON policy travels over an inherited pipe (never argv or the environment).
3. The launcher is single-threaded and runs before anything else. It applies the restrictions to
   itself and then `exec`s the command, so every descendant inherits them.

### Linux layers

| Layer | What it does | Needs |
|---|---|---|
| Namespaces (optional) | user + mount + pid + ipc + uts namespaces, plus a network namespace unless `network = "full"`. A tmpfs root assembled from bind mounts and `pivot_root`: read-only system and `$HOME` (minus secrets), read-write project, temp and caches, fresh `/proc`, minimal `/dev`, private `/tmp`. Secrets and project `.env*` files are masked (`/dev/null` over files, an empty read-only tmpfs over directories). ruckcode's own config is bind-mounted read-only. PID 1 inside reaps orphans; the whole tree dies with the launcher. | unprivileged user namespaces |
| Landlock (always, if present) | filesystem allow-list: read+exec on the system and `$HOME` children (secret directories excluded), full access to the project, temp dirs, tool caches and devices. With ABI 4 (Linux 6.7+) and no network namespace: TCP connect only to the proxy port (bind denied). | Linux 5.13+ (ABI 4 for network) |
| seccomp (always) | EPERM (not kill) for `ptrace`, `process_vm_*`, `mount` family, `unshare`/`setns`, `kexec`, `bpf`, `perf_event_open`, `keyctl`/`add_key`, module loading, `io_uring`, `userfaultfd`, `personality` changes; `socket()` for `AF_PACKET`, non-route `AF_NETLINK`, and (unless `network = "full"`) UDP/raw `AF_INET`/`AF_INET6`. `no_new_privs` is set and capabilities are dropped. | seccomp |

Network: commands get `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` (and lowercase, npm, cargo variants)
pointing at an in-process HTTP + CONNECT proxy with a domain allowlist.

* In a network namespace the only route out is a forwarder started by the launcher inside the
  namespace: `127.0.0.1:<port>` to a unix socket bind-mounted into the sandbox, served by the host
  proxy. Direct connections cannot leave.
* Without namespaces, Landlock ABI 4 restricts TCP `connect` to the proxy port; seccomp blocks UDP
  and raw sockets.
* On kernels with neither, the proxy is only advisory (`rc sandbox status` and a one-time warning say
  so); with `sandbox.strict = true` commands are refused.

### macOS

The launcher calls `sandbox_init_with_parameters` from libSystem (declared with `extern "C"`) with a
generated SBPL profile: writes only to the allowed roots, `file-read*`/`file-write*` denied on
secrets and masked `.env*` files, ruckcode's config not writable, network outbound only to
`localhost:<proxy port>` (and unix sockets). `.env*` files that exist at launch are denied by
the kernel.

### Native Windows

No sandbox: commands run unsandboxed and the first result says so ("sandbox unavailable on native
Windows; commands run unsandboxed — use WSL"). `sandbox.strict = true` refuses to run commands.
Commands through the WSL host run the Linux `rc` and are sandboxed there.

## Defaults

* Read: the whole system and your home directory, **except** `~/.ssh`, `~/.aws`, `~/.gnupg`,
  `~/.config/gh`, `~/.docker/config.json`, `~/.netrc`, `~/.git-credentials`, `~/.kube`, `~/.azure`,
  `~/.config/gcloud`, `~/.npmrc`, `~/.pypirc`, `~/.cargo/credentials*`, `~/.password-store`,
  `~/.local/share/keyrings` and ruckcode's `credentials.toml`.
* Write: the project root, `/tmp`, `/var/tmp`, `$TMPDIR`, and the tool caches that exist
  (`~/.cargo`, `~/.rustup`, `~/.npm`, `~/.cache`, `~/.local/share/pnpm`, `~/.local/share/mise`,
  `~/.yarn`, `~/go`, ...). Nothing else.
* Never writable by commands or by the Write/Edit tools: the project's `.ruckcode/` and `.mcp.json`,
  and ruckcode's user config directory. (The Read tool also refuses the secret paths above.)
* Network: `github.com`, `*.githubusercontent.com`, `crates.io`, `static.crates.io`,
  `index.crates.io`, `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`,
  `proxy.golang.org`, `sum.golang.org`. Any other domain prompts: **Allow** (once), **Always allow**
  (remembered for this project in the user data directory, outside anything a command can write) or
  **Reject**.
* Git over SSH (`git@github.com:...`) is not available; use HTTPS remotes.

## Configuration

User `config.toml` (`~/.config/ruckcode/config.toml`) and project `.ruckcode/config.toml`:

```toml
[sandbox]
enabled = true            # the only off switch; honoured ONLY from the user's config
strict = false            # refuse to run commands unless every layer is enforced
namespaces = "auto"       # "auto" | "required" | "off"
network = "allowlist"     # "allowlist" | "off" | "full"
allowed_domains = ["github.com", "*.githubusercontent.com", "crates.io", "static.crates.io",
                   "index.crates.io", "registry.npmjs.org", "pypi.org", "files.pythonhosted.org",
                   "proxy.golang.org", "sum.golang.org"]
extra_write_paths = []    # "~/", absolute or project-relative
extra_read_paths = []
deny_read_paths = []      # added to the built-in secret list
cache_paths = ["~/.cargo", "~/.rustup", "~/.npm", "~/.cache", "~/.local/share/pnpm", ...]
allowed_ports = []        # extra local TCP ports commands may connect to
mask_env_files = true     # mask project .env* files (kernel-enforced with namespaces/Seatbelt)
private_tmp = true        # namespaces: /tmp is a session-private directory
```

Per mode in `modes.toml` (the same keys except `enabled`; lists replace, they do not merge):

```toml
[modes.build.sandbox]
extra_write_paths = ["~/.config/mytool"]
network = "off"
```

`sandbox.enabled` in a *project* `.ruckcode/config.toml` is ignored (a project is writable by the
agent). `enabled` cannot be set per mode either.

## When something is blocked

There is no escape hatch for the model. The tool result says what was blocked (denied paths, denied
domains) and that the user can widen the policy in `config.toml` or `modes.toml`. Network domain
prompts are the exception: they only extend the allowlist inside the sandbox.

## `rc sandbox status`

Prints the Landlock ABI, seccomp, user namespaces (with the reason when unavailable), Seatbelt, the
active layers and the effective policy (`--json` for machines). Example on a stock Ubuntu 24.04:

```
landlock: yes (ABI v4; network rules: yes)
seccomp: yes
user namespaces: no (AppArmor restricts unprivileged user namespaces ...)
active layers: landlock filesystem, landlock network, seccomp
warning: namespaces unavailable (...): using Landlock + seccomp only; project .env files are not masked ...
```

Degraded systems run with what is available and warn once per session. `sandbox.strict = true`
refuses to run commands without full enforcement (Landlock or namespaces for the filesystem, seccomp,
and Landlock-net or a network namespace for the network). `sandbox.namespaces = "required"` refuses
without namespaces.

### Ubuntu 23.10+ and user namespaces

`kernel.apparmor_restrict_unprivileged_userns=1` lets `unshare(CLONE_NEWUSER)` succeed but strips
capabilities in the new namespace, so the namespace layer is unavailable and `rc` falls back to
Landlock + seccomp. To enable it, grant `userns` to the `rc` binary with an AppArmor profile (never
installed automatically). Adjust the path, save as `/etc/apparmor.d/rc`, then
`sudo apparmor_parser -r /etc/apparmor.d/rc`:

```
abi <abi/4.0>,
include <tunables/global>

profile rc /usr/local/bin/rc flags=(unconfined) {
  userns,
}
```

For the desktop app use its binary path. Other reasons namespaces can be unavailable are reported by
`rc sandbox status`: `user.max_user_namespaces=0`, Debian's `kernel.unprivileged_userns_clone=0`, or
a container whose seccomp profile (Docker's default) blocks `unshare`.

## Limits (be honest)

* Landlock is allow-list only and cannot carve a subpath out of an allowed directory. Secrets
  under `$HOME` are excluded by enumerating siblings; entries created *later* directly under such a
  directory (for example a new `~/.config/newtool`) are not readable. The project root cannot be
  split: **without namespaces (or on macOS before the file exists) project `.env*` files are readable
  by sandboxed commands**; ruckcode warns and, after each command, restores `.ruckcode/*` and
  `.mcp.json` if a command changed them (the Write/Edit tools refuse them outright). With
  namespaces both are kernel-enforced.
* Pathname unix sockets are not restricted by Landlock below ABI 6: on the Landlock-only path a
  command could connect to e.g. a docker socket. `/run` is not exposed and `SSH_AUTH_SOCK` is
  removed from the environment; the namespace layer does not mount `/run` at all.
* A hostname allowed in `allowed_domains` is trusted for any port; HTTPS is tunnelled (no TLS
  inspection, so domain fronting is not detected).
* Landlock ABI 4 network mode denies `bind`, so servers the command starts do not work without a
  network namespace (`allowed_ports` only adds `connect`).
* seccomp blocks `ptrace`, `unshare` and friends, so debuggers, rootless containers and browser
  sandboxes do not work inside it.
* The proxy refuses hostnames that resolve to loopback/link-local addresses (DNS rebinding) unless
  the allowlist entry is that literal IP or `localhost`.
