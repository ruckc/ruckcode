#!/usr/bin/env python3
"""Tests for build_site.py and site/install.sh (stdlib only): python3 -m unittest scripts/test_build_site.py"""

import base64
import contextlib
import io
import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import build_site  # noqa: E402

VERSION = "1.2.3"
BASE = "https://example.test/ruckcode"
PUB_TEXT = "untrusted comment: minisign public key: 8D3BE8D849F12684\nRWSEJvFJ2Og7jbc4PImteX/k1L1HzhHiaddOt44DsTv6oy+dKAhYM/5d\n"
PUBKEY = base64.b64encode(PUB_TEXT.encode()).decode()
SIG_TEXT = "untrusted comment: sig\nAAAA\ntrusted comment: version:1.2.3\nBBBB\n"
SIG = base64.b64encode(SIG_TEXT.encode()).decode()

APP_FILES = [
    f"ruckcode-app-{VERSION}-aarch64-apple-darwin.dmg",
    f"ruckcode-app-{VERSION}-aarch64-apple-darwin.app.tar.gz",
    f"ruckcode-app-{VERSION}-x86_64-apple-darwin.dmg",
    f"ruckcode-app-{VERSION}-x86_64-apple-darwin.app.tar.gz",
    f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc.msi",
    f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc-portable.zip",
    f"ruckcode-app-{VERSION}-x86_64-unknown-linux-gnu.AppImage",
    f"ruckcode-app-{VERSION}-x86_64-unknown-linux-gnu.deb",
]
UPDATER_FILES = [f for f in APP_FILES if f.endswith((".app.tar.gz", ".msi", "-portable.zip", ".AppImage"))]


def rc_name(triple):
    return f"rc-{VERSION}-{triple}." + ("zip" if "windows" in triple else "tar.gz")


def host_triple():
    m = platform.machine()
    if platform.system() == "Linux":
        return "x86_64-unknown-linux-musl" if m in ("x86_64", "AMD64") else "aarch64-unknown-linux-gnu"
    return None


def make_dist(root: Path, sigs=True, app_files=None) -> Path:
    dist = root / "dist"
    dist.mkdir()
    for t, _l, _e in build_site.RC_TARGETS:
        name = rc_name(t)
        if name.endswith(".tar.gz"):
            src = root / f"pkg-{t}"
            (src / f"rc-{VERSION}-{t}").mkdir(parents=True)
            (src / f"rc-{VERSION}-{t}" / "rc").write_text("#!/bin/sh\necho installed-rc\n")
            with tarfile.open(dist / name, "w:gz") as tf:
                tf.add(src / f"rc-{VERSION}-{t}", arcname=f"rc-{VERSION}-{t}")
        else:
            (dist / name).write_bytes(b"zip")
    for n in APP_FILES if app_files is None else app_files:
        (dist / n).write_bytes(n.encode() * 10)
    for p in list(dist.iterdir()):
        digest = hashlib.sha256(p.read_bytes()).hexdigest()
        (dist / f"{p.name}.sha256").write_text(f"{digest}  {p.name}\n")
        if sigs and (p.name in UPDATER_FILES or p.name.startswith("rc-")):  # sig only for updater bundles
            (dist / f"{p.name}.sig").write_text(SIG)
    return dist


def run_build(root, dist, signed=True, pubkey=PUBKEY, **kw):
    out = root / "site"
    args = ["--dist", str(dist), "--out", str(out), "--version", VERSION, "--base-url", BASE,
            "--notes", str(root / "notes.md"), "--pub-date", "2026-01-02T03:04:05Z"]
    if pubkey:
        args += ["--pubkey", pubkey]
    if signed:
        args += ["--signed"]
    (root / "notes.md").write_text("### Features\n- add **updates** (abc1234)\n- see [docs](https://x.y/z) <script>\n")
    # Capture the script's output: its errors are printed as `::error::` workflow commands, which
    # GitHub would otherwise turn into annotations for tests that fail on purpose.
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
        code = build_site.main(args)
    run_build.output = buf.getvalue()
    return out, code


class BuildSite(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def test_windows_only_release(self):
        """The current pipeline ships only the MSI: no darwin/linux keys or download cards."""
        msi = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc.msi"
        dist = make_dist(self.tmp, app_files=[msi])
        out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 0)
        tauri = json.loads((out / "latest.json").read_text())
        self.assertEqual(set(tauri["platforms"]), {"windows-x86_64", "windows-x86_64-msi"})
        index = (out / "index.html").read_text()
        self.assertIn(msi, index)
        self.assertNotIn("macOS, Apple Silicon", index)
        self.assertNotIn(".AppImage", index)
        self.assertIn("Windows only", index)
        # rc downloads for every target are unchanged
        rc = json.loads((out / "rc" / "latest.json").read_text())
        self.assertEqual(set(rc["targets"]), {t for t, _l, _e in build_site.RC_TARGETS})
        self.assertIn("rc-%s-aarch64-apple-darwin.tar.gz" % VERSION, index)

    def test_portable_zip_is_its_own_platform_entry(self):
        msi = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc.msi"
        zipn = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc-portable.zip"
        dist = make_dist(self.tmp, app_files=[msi, zipn])
        out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 0)
        p = json.loads((out / "latest.json").read_text())["platforms"]
        self.assertEqual(
            set(p), {"windows-x86_64", "windows-x86_64-msi", "windows-x86_64-portable"}
        )
        self.assertTrue(p["windows-x86_64"]["url"].endswith(".msi"))
        self.assertTrue(p["windows-x86_64-portable"]["url"].endswith(zipn))
        self.assertEqual(p["windows-x86_64-portable"]["signature"], SIG)

    def test_portable_zip_without_signature_is_an_error(self):
        zipn = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc-portable.zip"
        msi = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc.msi"
        dist = make_dist(self.tmp, app_files=[msi, zipn])
        (dist / f"{zipn}.sig").unlink()
        _out, code = run_build(self.tmp, dist)
        self.assertNotEqual(code, 0)

    def test_an_app_installer_without_signature_is_an_error(self):
        msi = f"ruckcode-app-{VERSION}-x86_64-pc-windows-msvc.msi"
        dist = make_dist(self.tmp, app_files=[msi])
        (dist / f"{msi}.sig").unlink()
        _out, code = run_build(self.tmp, dist)
        self.assertNotEqual(code, 0)

    def test_signed_site(self):
        dist = make_dist(self.tmp)
        out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 0)
        tauri = json.loads((out / "latest.json").read_text())
        self.assertEqual(tauri["version"], VERSION)
        self.assertEqual(tauri["pub_date"], "2026-01-02T03:04:05Z")
        self.assertIn("updates", tauri["notes"])
        self.assertEqual(set(tauri["platforms"]), {
                "darwin-aarch64", "darwin-aarch64-app", "darwin-x86_64", "darwin-x86_64-app",
                "windows-x86_64", "windows-x86_64-msi", "windows-x86_64-portable",
                "linux-x86_64", "linux-x86_64-appimage",
            },
        )
        for key, p in tauri["platforms"].items():
            self.assertEqual(p["signature"], SIG, key)
            self.assertTrue(p["url"].startswith(f"{BASE}/download/v{VERSION}/"), key)
            self.assertTrue((out / "download" / f"v{VERSION}" / p["url"].rsplit("/", 1)[1]).is_file(), key)
        p = tauri["platforms"]
        # The MSI is the only Windows installer: the generic key and the msi key both use it.
        self.assertNotIn("windows-x86_64-nsis", p)
        self.assertTrue(p["windows-x86_64"]["url"].endswith(".msi"))
        self.assertEqual(p["windows-x86_64"], p["windows-x86_64-msi"])
        # The portable zip is its own entry; MSI entries must never point at it.
        self.assertTrue(p["windows-x86_64-portable"]["url"].endswith("-x86_64-pc-windows-msvc-portable.zip"))
        self.assertNotEqual(p["windows-x86_64-portable"], p["windows-x86_64"])
        self.assertTrue(p["linux-x86_64-appimage"]["url"].endswith(".AppImage"))
        self.assertEqual(p["darwin-aarch64-app"], p["darwin-aarch64"])
        self.assertTrue((out / "download" / f"v{VERSION}" / f"{Path(p['windows-x86_64-msi']['url']).name}.sig").is_file())
        self.assertTrue(tauri["platforms"]["linux-x86_64"]["url"].endswith(".AppImage"))
        self.assertTrue(tauri["platforms"]["darwin-aarch64"]["url"].endswith(".app.tar.gz"))
        rc = json.loads((out / "rc" / "latest.json").read_text())
        self.assertEqual(set(rc["targets"]), {t for t, _l, _e in build_site.RC_TARGETS})
        for t, e in rc["targets"].items():
            f = out / "download" / f"v{VERSION}" / e["url"].rsplit("/", 1)[1]
            self.assertEqual(e["sha256"], hashlib.sha256(f.read_bytes()).hexdigest(), t)
            self.assertEqual(e["signature"], SIG)
        d = out / "download" / f"v{VERSION}"
        sums = (d / "SHA256SUMS").read_text()
        self.assertIn(rc_name("x86_64-unknown-linux-musl"), sums)
        self.assertNotIn(".sig", sums)
        index = (out / "index.html").read_text()
        self.assertIn(f"v{VERSION}", index)
        self.assertIn("curl -fsSL https://example.test/ruckcode/install.sh | sh", index)
        self.assertIn(".AppImage", index)
        self.assertIn("RWSEJvFJ2Og7", index)  # raw public key shown
        self.assertIn("<strong>updates</strong>", index)
        self.assertNotIn("<script>", index)  # notes are escaped
        install = (out / "install.sh").read_text()
        self.assertIn(PUBKEY, install)
        self.assertNotIn("@BASE_URL@", install)
        self.assertTrue(os.access(out / "install.sh", os.X_OK))

    def test_unsigned_site_has_no_manifests_or_signatures(self):
        dist = make_dist(self.tmp, sigs=True)
        out, code = run_build(self.tmp, dist, signed=False, pubkey=None)
        self.assertEqual(code, 0)
        self.assertFalse((out / "latest.json").exists())
        self.assertFalse((out / "rc" / "latest.json").exists())
        names = {p.name for p in (out / "download" / f"v{VERSION}").iterdir()}
        self.assertFalse(any(n.endswith(".sig") for n in names))
        self.assertIn("SHA256SUMS", names)
        index = (out / "index.html").read_text()
        self.assertIn("not configured", index)
        self.assertIn("PUBKEY=''", (out / "install.sh").read_text())

    def test_signed_requires_every_signature(self):
        dist = make_dist(self.tmp)
        (dist / f"{UPDATER_FILES[0]}.sig").unlink()
        _out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 1)

    def test_signed_requires_pubkey(self):
        dist = make_dist(self.tmp)
        _out, code = run_build(self.tmp, dist, pubkey=None)
        self.assertEqual(code, 1)

    def test_size_limits(self):
        dist = make_dist(self.tmp)
        old = build_site.MAX_FILE
        build_site.MAX_FILE = 50
        self.addCleanup(setattr, build_site, "MAX_FILE", old)
        _out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 1)
        build_site.MAX_FILE = old
        build_site.MAX_SITE = 10
        self.addCleanup(setattr, build_site, "MAX_SITE", 900 * 1000 * 1000)
        _out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 1)

    def test_bad_pubkey(self):
        dist = make_dist(self.tmp)
        _out, code = run_build(self.tmp, dist, pubkey="@@@not-base64")
        self.assertEqual(code, 1)


class Consistency(unittest.TestCase):
    """The update site URL is written down in several places; they must agree."""

    ROOT = HERE.parent

    def test_base_url_everywhere(self):
        import re

        expected = "https://ruckc.github.io/ruckcode"
        for rel in ("crates/ruckcode-cli/src/update.rs", "crates/ruckcode-app/src/update.rs"):
            text = (self.ROOT / rel).read_text()
            m = re.search(r'DEFAULT_BASE_URL: &str = "([^"]+)"', text)
            self.assertIsNotNone(m, rel)
            self.assertEqual(m.group(1), expected, rel)
        for name in ("ci.yml", "release.yml"):
            wf = (self.ROOT / ".github/workflows" / name).read_text()
            self.assertIn(f"UPDATE_BASE_URL: {expected}", wf, name)
        conf = json.loads((self.ROOT / "crates/ruckcode-app/tauri.conf.json").read_text())
        self.assertEqual(conf["plugins"]["updater"]["endpoints"], [f"{expected}/latest.json"])
        self.assertIn(expected, (self.ROOT / "site/install.sh").read_text())

    def test_rc_targets_match_the_release_matrix(self):
        wf = (self.ROOT / ".github/workflows/ci.yml").read_text()
        for triple, _label, _ext in build_site.RC_TARGETS:
            self.assertIn(f"target: {triple}", wf, triple)
        for _key, (triple, _suffix) in build_site.UPDATER_PLATFORMS.items():
            self.assertIn(f"target: {triple}", wf, triple)


@unittest.skipUnless(host_triple(), "install.sh test needs Linux")
class InstallScript(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        dist = make_dist(self.tmp)
        self.out, code = run_build(self.tmp, dist)
        self.assertEqual(code, 0)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.home = self.tmp / "home"
        self.home.mkdir()
        # Fake curl: maps the base URL onto the site directory (no network).
        curl = self.bin / "curl"
        curl.write_text(
            f"""#!/bin/sh
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2;; -*) shift;; *) url="$1"; shift;; esac; done
f="{self.out}/${{url#{BASE}/}}"
[ -f "$f" ] || exit 22
cp "$f" "$out"
"""
        )
        curl.chmod(0o755)

    def install(self, minisign=None, extra_env=None, script=None):
        env = {"PATH": f"{self.bin}:{os.environ['PATH']}", "HOME": str(self.home)}
        if minisign is not None:
            ms = self.bin / "minisign"
            ms.write_text(f"#!/bin/sh\necho \"$@\" > {self.tmp}/minisign.args\nexit {minisign}\n")
            ms.chmod(0o755)
        env.update(extra_env or {})
        return subprocess.run(["sh", str(script or self.out / "install.sh")], env=env, capture_output=True, text=True)

    def test_installs_latest_with_signature_check(self):
        r = self.install(minisign=0)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("signature OK", r.stdout)
        rc = self.home / ".local/bin/rc"
        self.assertEqual(subprocess.run([str(rc)], capture_output=True, text=True).stdout.strip(), "installed-rc")
        args = (self.tmp / "minisign.args").read_text()
        self.assertIn("-V", args)
        self.assertIn("-m", args)

    def test_bad_signature_aborts(self):
        r = self.install(minisign=1)
        self.assertEqual(r.returncode, 6, r.stdout)
        self.assertFalse((self.home / ".local/bin/rc").exists())

    def test_without_minisign_is_checksum_only(self):
        # A PATH with only the tools the script needs, so a real minisign is never picked up.
        tools = self.tmp / "tools"
        tools.mkdir()
        for name in ("sh", "uname", "mktemp", "sed", "awk", "head", "sha256sum", "tar", "gzip",
                     "find", "cp", "chmod", "mkdir", "mv", "rm", "cat", "base64", "dirname", "basename"):
            found = shutil.which(name)
            if found:
                (tools / name).symlink_to(found)
        r = self.install(extra_env={"PATH": f"{self.bin}:{tools}"})
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("minisign is not installed", r.stdout)
        self.assertTrue((self.home / ".local/bin/rc").exists())

    def test_checksum_mismatch(self):
        d = self.out / "download" / f"v{VERSION}"
        (d / f"{rc_name(host_triple())}.sha256").write_text("00  x\n")
        r = self.install(minisign=0)
        self.assertEqual(r.returncode, 4, r.stdout)
        self.assertIn("checksum mismatch", r.stdout)
        self.assertFalse((self.home / ".local/bin/rc").exists())

    def test_install_dir_and_pinned_version(self):
        r = self.install(minisign=0, extra_env={"RUCKCODE_INSTALL_DIR": str(self.tmp / "dest"), "RUCKCODE_VERSION": f"v{VERSION}"})
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertTrue((self.tmp / "dest/rc").exists())
        r = self.install(minisign=0, extra_env={"RUCKCODE_VERSION": "0.0.1"})
        self.assertEqual(r.returncode, 5)
        self.assertIn("only the latest release is hosted", r.stdout)

    def test_unsigned_site_installs_with_checksum_only(self):
        other = self.tmp / "u"
        other.mkdir()
        out, code = run_build(other, make_dist(other), signed=False, pubkey=None)
        self.assertEqual(code, 0)
        shutil.rmtree(self.out)
        shutil.copytree(out, self.out)
        r = self.install(minisign=1)  # a failing minisign must not even be consulted
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("publishes no signing key", r.stdout)


if __name__ == "__main__":
    unittest.main()
