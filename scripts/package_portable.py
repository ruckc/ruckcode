#!/usr/bin/env python3
"""Build the portable Windows zip of the desktop app (no installer, runs per-user).

  package_portable.py --exe ruckcode.exe --version 1.2.3 --target x86_64-pc-windows-msvc \
      --out dist [--resource SRC=DEST ...]

Writes <out>/ruckcode-app-<version>-<target>-portable.zip containing one folder
`ruckcode-app-<version>/` with the exe, any bundled resources (DEST is relative to the exe, e.g.
wsl/rc, which is where Tauri looks for resources of an installed app), a `portable.txt` marker
(the app then updates itself in place from the portable zip instead of through the MSI) and a short README.
Checksum/signature are produced by the calling workflow, same as for the MSI.
"""
import argparse
import sys
import zipfile
from pathlib import Path

README = """ruckcode {version} (portable, {target})

Run ruckcode.exe. Nothing is installed, no administrator rights are needed; settings and
sessions live in your user profile exactly like the installed app.

Requirements: the Microsoft Edge WebView2 runtime (preinstalled on Windows 10/11).
This build updates itself like the installed app (Settings, check for updates): it downloads
the new signed zip, verifies it and swaps the files in this folder, so the folder must be
writable. No installer, no administrator rights. Leftover *.old files are removed on the next
start. Keep portable.txt: it tells the app to update in place. If an update ever fails you can
still download a zip from https://ruckc.github.io/ruckcode/ and replace this folder.
The executable is not code-signed, so SmartScreen may show "Windows protected your PC"
on first launch: More info, then Run anyway.
Verify the download against SHA256SUMS on the download page.
"""


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--target", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--resource", action="append", default=[], metavar="SRC=DEST")
    a = ap.parse_args()

    exe = Path(a.exe)
    if not exe.is_file() or exe.stat().st_size == 0:
        print(f"error: exe not found: {exe}", file=sys.stderr)
        return 1
    root = f"ruckcode-app-{a.version}"
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    zpath = out / f"{root}-{a.target}-portable.zip"

    entries = [(exe, "ruckcode.exe")]
    for r in a.resource:
        src, _, dest = r.partition("=")
        if not dest or not Path(src).is_file():
            print(f"error: bad --resource {r!r}", file=sys.stderr)
            return 1
        entries.append((Path(src), dest))

    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for src, dest in entries:
            z.write(src, f"{root}/{dest}")
        z.writestr(f"{root}/portable.txt", "Marker: this copy is portable; it updates itself in place from the portable zip.\n")
        z.writestr(f"{root}/README.txt", README.format(version=a.version, target=a.target))
    print(zpath)
    return 0


if __name__ == "__main__":
    sys.exit(main())
