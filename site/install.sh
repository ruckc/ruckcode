#!/bin/sh
# ruckcode `rc` installer for Linux, macOS and WSL.
#
#   curl -fsSL https://ruckc.github.io/ruckcode/install.sh | sh
#
# Downloads the latest `rc` release from the ruckcode site (GitHub Pages), verifies its SHA-256
# checksum and, when `minisign` is installed, its minisign signature, then installs it into
# ~/.local/bin (override with RUCKCODE_INSTALL_DIR). Without `minisign` only the checksum is
# verified (it comes from the same server, so it detects corruption, not a compromised server).
#
# Environment: RUCKCODE_VERSION (pin a version; only the latest is hosted), RUCKCODE_INSTALL_DIR,
# RUCKCODE_UPDATE_BASE_URL (testing/mirrors).
#
# The three @...@ tokens below are filled in when the site is built (and by the desktop app when
# it installs rc into WSL). Exit codes: 2 unsupported platform, 3 missing tool, 4 checksum
# mismatch, 5 download failed, 6 signature check failed.
set -eu

BASE_URL="${RUCKCODE_UPDATE_BASE_URL:-@BASE_URL@}"
PUBKEY='@PUBKEY@'
DEFAULT_VERSION='@VERSION@'
# An unreplaced token (running the template directly) means "not configured".
case "$PUBKEY" in "@"*) PUBKEY="" ;; esac
case "$DEFAULT_VERSION" in "@"*) DEFAULT_VERSION="" ;; esac
case "$BASE_URL" in "@"*) BASE_URL="https://ruckc.github.io/ruckcode" ;; esac
BASE_URL="${BASE_URL%/}"

say() { printf '[rc-install] %s\n' "$*"; }
die() { code="$1"; shift; say "error: $*"; exit "$code"; }

os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
  Linux-x86_64|Linux-amd64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  *) die 2 "unsupported platform $os $arch (Windows: use the installer from $BASE_URL/)" ;;
esac
say "platform $os $arch, target $target"

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  die 3 "neither curl nor wget is installed"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

ver="${RUCKCODE_VERSION:-}"
ver="${ver#v}"
if [ -z "$ver" ]; then
  if fetch "$BASE_URL/rc/latest.json" "$tmp/latest.json" 2>/dev/null; then
    ver=$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)
  fi
fi
[ -n "$ver" ] || ver="$DEFAULT_VERSION"
[ -n "$ver" ] || die 5 "could not determine the latest version from $BASE_URL/rc/latest.json"
case "$ver" in
  *[!0-9A-Za-z.+-]*|"") die 5 "invalid version '$ver'" ;;
esac

archive="rc-$ver-$target.tar.gz"
dl="$BASE_URL/download/v$ver"
say "downloading $archive"
fetch "$dl/$archive" "$tmp/$archive" || die 5 "download failed ($dl/$archive); only the latest release is hosted"

say "verifying checksum"
fetch "$dl/$archive.sha256" "$tmp/archive.sha256" || die 5 "checksum download failed"
want=$(awk '{ print $1 }' "$tmp/archive.sha256" | head -n 1)
if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
else die 3 "no sha256sum or shasum available"; fi
[ -n "$want" ] && [ "$want" = "$got" ] || die 4 "checksum mismatch (expected $want, got $got)"

if [ -z "$PUBKEY" ]; then
  say "this site publishes no signing key; skipped the signature check (checksum only)"
elif command -v minisign >/dev/null 2>&1; then
  say "verifying minisign signature"
  fetch "$dl/$archive.sig" "$tmp/archive.sig" || die 6 "signature download failed"
  # Both files are base64 of the minisign text (the format `tauri signer` writes).
  b64d() { base64 -d 2>/dev/null || base64 -D; }
  printf '%s' "$PUBKEY" | b64d > "$tmp/key.pub" || die 6 "cannot decode the embedded public key"
  b64d < "$tmp/archive.sig" > "$tmp/archive.minisig" || die 6 "cannot decode the signature"
  minisign -V -q -p "$tmp/key.pub" -x "$tmp/archive.minisig" -m "$tmp/$archive" >/dev/null 2>&1 \
    || die 6 "signature verification failed; nothing was installed"
  say "signature OK"
else
  say "minisign is not installed: verified the checksum only (install minisign to verify the signature)"
fi

say "extracting"
tar -xzf "$tmp/$archive" -C "$tmp"
bin=$(find "$tmp" -type f -name rc | head -n 1)
[ -n "$bin" ] || die 5 "the archive does not contain rc"

dir="${RUCKCODE_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dir"
cp "$bin" "$dir/rc.new"
chmod 755 "$dir/rc.new"
mv -f "$dir/rc.new" "$dir/rc"
say "installed $dir/rc (version $ver)"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) say "note: $dir is not on your PATH; add it to your shell profile" ;;
esac
