#!/usr/bin/env python3
"""Assemble the GitHub Pages site for a ruckcode release (latest version only).

Layout of the output directory:

    index.html                       download page (version, notes, links, checksums)
    install.sh                       `curl | sh` installer for rc (Linux, macOS, WSL)
    latest.json                      Tauri updater manifest      (only when signed)
    rc/latest.json                   `rc self-update` manifest   (only when signed)
    download/v<version>/<files...>   every release artifact (+ .sha256, .sig, SHA256SUMS)

The repository is private, so GitHub Release assets are not publicly downloadable; this site is
where users, the desktop updater and `rc self-update` fetch releases from.

"Signed" means the minisign signatures (`<file>.sig`, made with the Tauri updater key) exist for
every updater bundle and rc archive. Without them no manifest is written, so the app and `rc`
report "updates not configured" instead of offering unverifiable downloads.

Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import html
import json
import re
import shutil
import sys
from pathlib import Path

MAX_FILE = 100 * 1000 * 1000  # per file, GitHub's hard limit is 100 MiB
MAX_SITE = 900 * 1000 * 1000  # whole site, Pages limit is 1 GB

RC_TARGETS = [
    ("x86_64-unknown-linux-musl", "Linux x86_64", "tar.gz"),
    ("aarch64-unknown-linux-gnu", "Linux aarch64", "tar.gz"),
    ("aarch64-apple-darwin", "macOS Apple Silicon", "tar.gz"),
    ("x86_64-apple-darwin", "macOS Intel", "tar.gz"),
    ("x86_64-pc-windows-msvc", "Windows x86_64", "zip"),
]

# Tauri platform key -> (app target triple, updater bundle suffix). The plugin looks up
# `{os}-{arch}-{bundle}` first (bundle = the installer type the running copy was installed from:
# msi, appimage, app, deb, rpm) and then the generic `{os}-{arch}`. The MSI is the only Windows
# installer, so both Windows keys point at it. .deb/.rpm are never listed: they update by hand.
UPDATER_PLATFORMS = {
    "darwin-aarch64": ("aarch64-apple-darwin", ".app.tar.gz"),
    "darwin-aarch64-app": ("aarch64-apple-darwin", ".app.tar.gz"),
    "darwin-x86_64": ("x86_64-apple-darwin", ".app.tar.gz"),
    "darwin-x86_64-app": ("x86_64-apple-darwin", ".app.tar.gz"),
    "windows-x86_64": ("x86_64-pc-windows-msvc", ".msi"),
    "windows-x86_64-msi": ("x86_64-pc-windows-msvc", ".msi"),
    "linux-x86_64": ("x86_64-unknown-linux-gnu", ".AppImage"),
    "linux-x86_64-appimage": ("x86_64-unknown-linux-gnu", ".AppImage"),
}

APP_ROWS = [
    ("macOS, Apple Silicon", "aarch64-apple-darwin", [(".dmg", "Disk image (.dmg)")]),
    ("macOS, Intel", "x86_64-apple-darwin", [(".dmg", "Disk image (.dmg)")]),
    (
        "Windows, x64",
        "x86_64-pc-windows-msvc",
        [
            (".msi", "MSI installer (.msi, per-user by default; all users: msiexec /i ... ALLUSERS=1)"),
            ("-portable.zip", "Portable zip (no installer; unzip and run ruckcode.exe, no updates)"),
        ],
    ),
    (
        "Linux, x86_64",
        "x86_64-unknown-linux-gnu",
        [(".AppImage", "AppImage (self-updating)"), (".deb", "Debian package (.deb)")],
    ),
]


class SiteError(Exception):
    pass


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def human(n: int) -> str:
    size = float(n)
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1000 or unit == "GB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1000
    return f"{n} B"


def raw_pubkey(pubkey_b64: str) -> str:
    """The bare `RW...` key from the Tauri-style base64 of the minisign .pub file."""
    s = pubkey_b64.strip()
    if s.startswith("RW"):
        return s
    try:
        text = base64.b64decode(s, validate=True).decode()
    except Exception as e:  # noqa: BLE001
        raise SiteError(f"RUCKCODE_UPDATER_PUBKEY is not valid base64: {e}") from e
    lines = [ln.strip() for ln in text.splitlines() if ln.strip()]
    if len(lines) < 2 or not lines[1].startswith("RW"):
        raise SiteError("RUCKCODE_UPDATER_PUBKEY does not look like a minisign public key")
    return lines[1]


# ---- tiny markdown -> html (git-cliff notes: headings, bullets, emphasis, links) ----------


def inline_md(text: str) -> str:
    t = html.escape(text, quote=False)
    t = re.sub(r"`([^`]+)`", r"<code>\1</code>", t)
    t = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", t)

    def link(m: re.Match[str]) -> str:
        url = m.group(2)
        if not re.match(r"https?://", url):
            return m.group(0)
        return f'<a href="{html.escape(url, quote=True)}" rel="noopener">{m.group(1)}</a>'

    return re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, t)


def markdown_to_html(md: str) -> str:
    out: list[str] = []
    in_list = False
    para: list[str] = []

    def flush_para() -> None:
        if para:
            out.append("<p>" + inline_md(" ".join(para)) + "</p>")
            para.clear()

    def close_list() -> None:
        nonlocal in_list
        if in_list:
            out.append("</ul>")
            in_list = False

    for raw in md.splitlines():
        line = raw.rstrip()
        m_head = re.match(r"^(#{1,6})\s+(.*)$", line)
        m_item = re.match(r"^\s*[-*]\s+(.*)$", line)
        if not line.strip():
            flush_para()
            close_list()
        elif m_head:
            flush_para()
            close_list()
            level = min(len(m_head.group(1)) + 2, 6)  # page title is h1, sections h2
            out.append(f"<h{level}>{inline_md(m_head.group(2))}</h{level}>")
        elif m_item:
            flush_para()
            if not in_list:
                out.append("<ul>")
                in_list = True
            out.append(f"<li>{inline_md(m_item.group(1))}</li>")
        else:
            close_list()
            para.append(line.strip())
    flush_para()
    close_list()
    return "\n".join(out)


# ---- the site -----------------------------------------------------------------------------


def find_artifacts(dist: Path) -> list[Path]:
    files = sorted(p for p in dist.iterdir() if p.is_file())
    if not files:
        raise SiteError(f"no artifacts found in {dist}")
    return files


def build_manifests(
    version: str, notes: str, pub_date: str, base: str, dist: Path
) -> tuple[dict, dict]:
    """(Tauri updater manifest, rc manifest); raises if any signed artifact is missing."""
    dl = f"{base}/download/v{version}"
    problems: list[str] = []

    def sig_of(name: str) -> str:
        sig = dist / f"{name}.sig"
        if not (dist / name).is_file():
            problems.append(f"missing {name}")
            return ""
        if not sig.is_file():
            problems.append(f"missing {name}.sig")
            return ""
        return sig.read_text().strip()

    # Only platforms whose installer was built appear (the release currently ships the Windows
    # MSI only); a present installer without its .sig is still an error.
    platforms = {}
    for key, (triple, suffix) in UPDATER_PLATFORMS.items():
        name = f"ruckcode-app-{version}-{triple}{suffix}"
        if not (dist / name).is_file():
            continue
        platforms[key] = {"signature": sig_of(name), "url": f"{dl}/{name}"}
    if not platforms:
        problems.append("no desktop app installer found")

    targets = {}
    for triple, _label, ext in RC_TARGETS:
        name = f"rc-{version}-{triple}.{ext}"
        sig = sig_of(name)
        sha = ""
        if (dist / name).is_file():
            sha = sha256_file(dist / name)
        targets[triple] = {"url": f"{dl}/{name}", "sha256": sha, "signature": sig}

    if problems:
        raise SiteError("cannot write updater manifests: " + "; ".join(problems))

    tauri = {"version": version, "notes": notes, "pub_date": pub_date, "platforms": platforms}
    rc = {"version": version, "notes": notes, "pub_date": pub_date, "targets": targets}
    return tauri, rc


CSS = """
:root{color-scheme:light dark;--bg:#fbfbfa;--fg:#1c1c1a;--muted:#62625c;--card:#ffffff;
--line:#dcdcd5;--accent:#1f5fbf;--code:#efeee9}
@media (prefers-color-scheme:dark){:root{--bg:#161614;--fg:#ecece7;--muted:#a3a39b;
--card:#1f1f1c;--line:#34342f;--accent:#7fb0ff;--code:#272724}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,
"Segoe UI",Roboto,sans-serif}
main{max-width:56rem;margin:0 auto;padding:2rem 16px 4rem}
h1{font-size:1.9rem;margin:.2rem 0}h2{margin-top:2.2rem;font-size:1.3rem}
h3,h4{margin:1.1rem 0 .3rem}
a{color:var(--accent)}code,pre{background:var(--code);border-radius:6px;font:.88em ui-monospace,
SFMono-Regular,Menlo,Consolas,monospace}
code{padding:.1em .35em}pre{padding:.8rem 1rem;overflow-x:auto}pre code{padding:0}
.sub{color:var(--muted)}
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(15rem,1fr));gap:12px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px}
.card h3{margin-top:0}.card ul{margin:.3rem 0 0;padding-left:1.1rem}
.hash{color:var(--muted);font:.78em ui-monospace,monospace;word-break:break-all}
table{width:100%;border-collapse:collapse;font-size:.92rem}
th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);vertical-align:top}
td.n{word-break:break-all}
.notes{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:4px 18px}
.warn{border-left:4px solid var(--accent);padding:.1rem 1rem;background:var(--card)}
footer{margin-top:3rem;color:var(--muted);font-size:.9rem}
"""


def render_index(
    version: str,
    pub_date: str,
    notes_md: str,
    base: str,
    dist: Path,
    files: list[Path],
    sums: dict[str, str],
    pubkey_b64: str | None,
    signed: bool,
) -> str:
    dl = f"{base}/download/v{version}"
    names = {p.name for p in files}

    def link(name: str, label: str) -> str:
        if name not in names:
            return ""
        sha = sums[name]
        extra = ""
        if f"{name}.sig" in names:
            extra = f' · <a href="{dl}/{html.escape(name)}.sig">signature</a>'
        return (
            f'<li><a href="{dl}/{html.escape(name)}">{html.escape(label)}</a> '
            f'<span class="sub">({human((dist / name).stat().st_size)})</span>{extra}'
            f'<br><span class="hash">sha256 {sha}</span></li>'
        )

    app_cards = []
    for title, triple, kinds in APP_ROWS:
        items = "".join(
            link(f"ruckcode-app-{version}-{triple}{suffix}", label) for suffix, label in kinds
        )
        if items:
            app_cards.append(f'<div class="card"><h3>{html.escape(title)}</h3><ul>{items}</ul></div>')

    have_app = lambda suffix: any(n.startswith(f"ruckcode-app-{version}-") and n.endswith(suffix) for n in names)  # noqa: E731
    app_note = "The desktop app is currently available for Windows only; on macOS and Linux use the command line tool below."
    if have_app(".dmg") or have_app(".AppImage") or have_app(".deb"):
        app_note = (
            "Builds are not yet code-signed: macOS Gatekeeper and Windows SmartScreen will warn on "
            "first launch. On Linux only the AppImage updates itself; for the .deb, download the "
            "new version from this page."
        )
    elif have_app(".msi"):
        app_note = (
            "Builds are not yet code-signed: Windows SmartScreen will warn on first launch. "
            "The installer includes <code>rc</code> and installs it into WSL2 distributions "
            "automatically. The desktop app is currently Windows only; on macOS and Linux use the "
            "command line tool below."
        )
    rc_cards = []
    for triple, label, ext in RC_TARGETS:
        item = link(f"rc-{version}-{triple}.{ext}", f"rc-{version}-{triple}.{ext}")
        if item:
            rc_cards.append(f'<div class="card"><h3>{html.escape(label)}</h3><ul>{item}</ul></div>')

    rows = "".join(
        f'<tr><td class="n"><a href="{dl}/{html.escape(p.name)}">{html.escape(p.name)}</a></td>'
        f"<td>{human(p.stat().st_size)}</td></tr>"
        for p in files
    )

    if signed and pubkey_b64:
        raw = raw_pubkey(pubkey_b64)
        verify = f"""
<h2 id="verify">Verify a download</h2>
<p>Every file has a <code>.sha256</code> next to it and <a href="{dl}/SHA256SUMS">SHA256SUMS</a>
lists them all. Updater bundles and <code>rc</code> archives are also signed with
<a href="https://jedisct1.github.io/minisign/">minisign</a> (<code>.sig</code> files; they hold
the base64 form, so decode first):</p>
<pre><code>base64 -d rc-{version}-x86_64-unknown-linux-musl.tar.gz.sig &gt; rc.minisig
minisign -V -P {html.escape(raw)} -x rc.minisig -m rc-{version}-x86_64-unknown-linux-musl.tar.gz</code></pre>
<p class="sub">Public key: <code>{html.escape(raw)}</code></p>"""
        install_note = (
            "The installer verifies the checksum and, if <code>minisign</code> is installed, the "
            "signature."
        )
        update_note = (
            "The desktop app updates itself in the background; <code>rc self-update</code> updates "
            "the command line tool. Both verify signatures."
        )
    else:
        verify = f"""
<h2 id="verify">Verify a download</h2>
<p>Every file has a <code>.sha256</code> next to it and <a href="{dl}/SHA256SUMS">SHA256SUMS</a>
lists them all. This release was built without update signing, so there are no signatures and
automatic updates are not offered.</p>"""
        install_note = "The installer verifies the SHA-256 checksum."
        update_note = "Automatic updates are not configured for this release."

    notes_html = markdown_to_html(notes_md) if notes_md.strip() else "<p>No notes.</p>"
    date_h = pub_date[:10]
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Download ruckcode</title>
<meta name="description" content="Download ruckcode {html.escape(version)}: the desktop app and the rc command line tool.">
<style>{CSS}</style>
</head>
<body>
<main>
<h1>ruckcode</h1>
<p class="sub">Latest release <strong>v{html.escape(version)}</strong>, {html.escape(date_h)}.
Older versions are not hosted here.</p>

<h2 id="app">Desktop app</h2>
<div class="cards">{"".join(app_cards)}</div>
<p class="sub">{update_note} {app_note}</p>

<h2 id="rc">Command line: rc</h2>
<p>Linux, macOS and WSL:</p>
<pre><code>curl -fsSL {html.escape(base)}/install.sh | sh</code></pre>
<p class="sub">{install_note} It installs into <code>~/.local/bin</code>
(<code>RUCKCODE_INSTALL_DIR</code> to change). Later, run <code>rc self-update</code>.</p>
<div class="cards">{"".join(rc_cards)}</div>
{verify}

<h2 id="notes">Release notes</h2>
<div class="notes">{notes_html}</div>

<h2 id="files">All files</h2>
<table><thead><tr><th>File</th><th>Size</th></tr></thead><tbody>{rows}</tbody></table>

<footer>Source: <a href="https://github.com/ruckc/ruckcode">github.com/ruckc/ruckcode</a>.
This page is generated from the release workflow.</footer>
</main>
</body>
</html>
"""


def build(args: argparse.Namespace) -> None:
    version = args.version.lstrip("v")
    base = args.base_url.rstrip("/")
    dist = Path(args.dist)
    out = Path(args.out)
    template = Path(args.install_template)
    notes_md = Path(args.notes).read_text() if args.notes and Path(args.notes).is_file() else ""
    pub_date = args.pub_date or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    pubkey = (args.pubkey or "").strip() or None

    files = find_artifacts(dist)
    signed = bool(pubkey) and args.signed
    if args.signed and not pubkey:
        raise SiteError("signing is enabled but no public key (RUCKCODE_UPDATER_PUBKEY) was given")
    if not signed:
        files = [p for p in files if p.suffix != ".sig"]

    if out.exists():
        shutil.rmtree(out)
    dest = out / "download" / f"v{version}"
    dest.mkdir(parents=True)
    for p in files:
        shutil.copy2(p, dest / p.name)

    sums = {p.name: sha256_file(p) for p in files}
    # Per-file checksums already exist for most artifacts; SHA256SUMS covers everything (not .sig).
    listed = sorted(n for n in sums if not n.endswith((".sig", ".sha256")) and n != "SHA256SUMS")
    (dest / "SHA256SUMS").write_text("".join(f"{sums[n]}  {n}\n" for n in listed))
    sums["SHA256SUMS"] = sha256_file(dest / "SHA256SUMS")
    files = sorted(dest.iterdir())

    if signed:
        tauri, rc = build_manifests(version, notes_md, pub_date, base, dist)
        (out / "latest.json").write_text(json.dumps(tauri, indent=2) + "\n")
        (out / "rc").mkdir()
        (out / "rc" / "latest.json").write_text(json.dumps(rc, indent=2) + "\n")

    script = template.read_text()
    for token, value in (
        ("@BASE_URL@", base),
        ("@PUBKEY@", pubkey if signed and pubkey else ""),
        ("@VERSION@", version),
    ):
        script = script.replace(token, value)
    (out / "install.sh").write_text(script)
    (out / "install.sh").chmod(0o755)

    (out / "index.html").write_text(
        render_index(version, pub_date, notes_md, base, dest, files, sums, pubkey, signed)
    )

    check_limits(out)
    total = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
    print(f"site: {len(files)} release files, {human(total)} total, signed={signed}")


def check_limits(out: Path) -> None:
    total = 0
    for p in out.rglob("*"):
        if not p.is_file():
            continue
        size = p.stat().st_size
        total += size
        if size >= MAX_FILE:
            raise SiteError(
                f"{p.relative_to(out)} is {human(size)}; GitHub Pages files must stay under "
                f"{human(MAX_FILE)}. Shrink it or stop publishing it on Pages."
            )
    if total >= MAX_SITE:
        raise SiteError(
            f"the site is {human(total)}; it must stay under {human(MAX_SITE)} (Pages limit 1 GB)"
        )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--dist", required=True, help="directory with all release artifacts")
    ap.add_argument("--out", required=True, help="output directory (recreated)")
    ap.add_argument("--version", required=True)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--notes", help="release notes markdown file")
    ap.add_argument("--pubkey", help="updater public key (Tauri base64 of the minisign .pub)")
    ap.add_argument(
        "--signed",
        action="store_true",
        help="publish updater manifests; fail if any .sig is missing",
    )
    ap.add_argument("--pub-date", help="RFC 3339 timestamp (default: now)")
    ap.add_argument(
        "--install-template", default=str(Path(__file__).resolve().parent.parent / "site/install.sh")
    )
    args = ap.parse_args(argv)
    try:
        build(args)
    except SiteError as e:
        print(f"::error::{e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
