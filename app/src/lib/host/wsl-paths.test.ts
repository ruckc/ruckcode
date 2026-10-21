import { describe, expect, it } from 'vitest';
import { isWindowsPath, windowsToLinux } from './wsl-paths';
import { distroLabel, versionMatches } from './wsl';

const lin = (p: string, d = 'Ubuntu-24.04') => {
  const t = windowsToLinux(p, d);
  return t.ok ? t.linux : `ERR:${t.error}`;
};

// Mirrors the cases in crates/ruckcode-app/src/wsl.rs so both implementations agree.
describe('windowsToLinux', () => {
  it('translates WSL UNC shares', () => {
    expect(lin('\\\\wsl$\\Ubuntu-24.04\\home\\x')).toBe('/home/x');
    expect(lin('\\\\wsl.localhost\\Ubuntu-24.04\\home\\x\\')).toBe('/home/x');
    expect(lin('\\\\WSL.LOCALHOST\\ubuntu-24.04\\home\\x')).toBe('/home/x');
    expect(lin('\\\\wsl$\\Ubuntu-24.04')).toBe('/');
    expect(lin('//wsl$/Ubuntu-24.04/a//b')).toBe('/a/b');
    expect(lin('\\\\?\\UNC\\wsl.localhost\\Ubuntu-24.04\\srv')).toBe('/srv');
  });

  it('rejects other distros, bare shares and network paths', () => {
    expect(lin('\\\\wsl$\\Debian\\home\\x')).toMatch(/ERR:.*Debian.*Ubuntu-24.04/);
    expect(lin('\\\\wsl$')).toMatch(/^ERR:/);
    expect(lin('\\\\fileserver\\share\\x')).toMatch(/ERR:.*network/);
  });

  it('maps drive letters under /mnt, case-insensitively', () => {
    expect(lin('C:\\Users\\x\\proj')).toBe('/mnt/c/Users/x/proj');
    expect(lin('c:\\Users\\x')).toBe('/mnt/c/Users/x');
    expect(lin('D:/work/a b/')).toBe('/mnt/d/work/a b');
    expect(lin('E:\\')).toBe('/mnt/e');
    expect(lin('e:')).toBe('/mnt/e');
    expect(lin('\\\\?\\C:\\x')).toBe('/mnt/c/x');
    expect(lin('C:foo')).toMatch(/^ERR:/);
  });

  it('passes Linux paths through', () => {
    expect(lin('/home/x/proj')).toBe('/home/x/proj');
    expect(lin('/home/x/proj/')).toBe('/home/x/proj');
    expect(lin('/')).toBe('/');
    expect(lin('  /mnt/c/x  ')).toBe('/mnt/c/x');
    expect(lin('~/code')).toBe('~/code');
    expect(lin('relative/dir')).toMatch(/^ERR:/);
    expect(lin('   ')).toMatch(/^ERR:/);
  });

  it('tells Windows paths from Linux ones', () => {
    expect(isWindowsPath('C:\\x')).toBe(true);
    expect(isWindowsPath('\\\\wsl$\\U\\x')).toBe(true);
    expect(isWindowsPath('/home/x')).toBe(false);
  });
});

describe('rc status helpers', () => {
  it('compares versions ignoring build suffixes', () => {
    const rc = (version: string | null, installed = true) => ({
      installed,
      path: '/rc',
      version,
      expected: '0.1.0'
    });
    expect(versionMatches(rc('0.1.0'))).toBe(true);
    expect(versionMatches(rc('0.1.0-3-gabc'))).toBe(true);
    expect(versionMatches(rc('0.0.9'))).toBe(false);
    expect(versionMatches(rc(null))).toBe(false);
    expect(versionMatches(rc('0.1.0', false))).toBe(false);
  });

  it('labels distros', () => {
    expect(
      distroLabel({
        name: 'U',
        state: 'Running',
        version: 2,
        default: true,
        selectable: true,
        reason: null
      })
    ).toBe('U (default, Running, WSL2)');
  });
});
