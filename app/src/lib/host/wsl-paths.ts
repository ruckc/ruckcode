/**
 * Windows <-> Linux path translation for WSL2. A TypeScript twin of `windows_to_linux` /
 * `linux_to_windows` in `crates/ruckcode-app/src/wsl.rs`: the desktop app asks the backend (which
 * can use `wslpath`), the browser mock uses this, and both share the same test cases.
 */

export type Translation = { ok: true; linux: string } | { ok: false; error: string };

function collapse(path: string): string {
  const segs = path.split('/').filter(Boolean);
  return '/' + segs.join('/');
}

/** Translate a Windows or Linux path to the Linux path to use inside `distro`. */
export function windowsToLinux(input: string, distro: string): Translation {
  const fail = (error: string): Translation => ({ ok: false, error });
  let p = input.trim();
  if (!p) return fail('path is empty');
  if (/^\\\\\?\\UNC\\/i.test(p)) p = '\\\\' + p.slice(8);
  else if (p.startsWith('\\\\?\\')) p = p.slice(4);
  const norm = p.replace(/\\/g, '/');

  if (norm.startsWith('//')) {
    const [host = '', share = '', ...rest] = norm.slice(2).split('/');
    const h = host.toLowerCase();
    if (h === 'wsl$' || h === 'wsl.localhost') {
      if (!share) return fail('the WSL share path must name a distro');
      if (share.toLowerCase() !== distro.toLowerCase()) {
        return fail(`path belongs to distro \`${share}\`, but \`${distro}\` is selected`);
      }
      return { ok: true, linux: collapse(rest.join('/')) };
    }
    return fail(
      `network path \`${input.trim()}\` is not reachable from WSL; use a drive letter or a path inside the distro`
    );
  }
  const drive = /^([A-Za-z]):(.*)$/.exec(norm);
  if (drive) {
    const tail = drive[2];
    if (tail && !tail.startsWith('/')) {
      return fail(`drive-relative path \`${input.trim()}\` is not supported`);
    }
    const t = collapse(tail);
    const letter = drive[1].toLowerCase();
    return { ok: true, linux: t === '/' ? `/mnt/${letter}` : `/mnt/${letter}${t}` };
  }
  if (norm.startsWith('/')) {
    const t = norm.length > 1 ? norm.replace(/\/+$/, '') : norm;
    return { ok: true, linux: t || '/' };
  }
  if (norm === '~' || norm.startsWith('~/')) {
    return { ok: true, linux: norm.replace(/\/+$/, '') };
  }
  return fail(`\`${input.trim()}\` is not an absolute path`);
}

/** Does this look like a Windows path (drive or UNC) rather than a Linux one? */
export function isWindowsPath(p: string): boolean {
  const t = p.trim();
  return /^[A-Za-z]:([\\/]|$)/.test(t) || t.startsWith('\\\\');
}
