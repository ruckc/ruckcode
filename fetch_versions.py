import urllib.request
import json
import sys

def get_npm_version(package):
    try:
        url = f"https://registry.npmjs.org/{package}/latest"
        with urllib.request.urlopen(url) as response:
            data = json.loads(response.read().decode())
            return data.get("version")
    except Exception as e:
        return None

def get_crate_version(crate):
    try:
        url = f"https://crates.io/api/v1/crates/{crate}"
        with urllib.request.urlopen(url) as response:
            data = json.loads(response.read().decode())
            return data["crate"]["max_version"]
    except Exception as e:
        return None

npm_packages = [
    "@agentclientprotocol/sdk", "@ruckc/truss-chat", "@ruckc/truss-code",
    "@ruckc/truss-markdown", "@ruckc/truss-ui", "@tauri-apps/api",
    "@tauri-apps/plugin-dialog", "diff", "zod", "@eslint/js",
    "@playwright/test", "@sveltejs/adapter-static", "@sveltejs/kit",
    "@sveltejs/vite-plugin-svelte", "@tauri-apps/cli", "@testing-library/jest-dom",
    "@testing-library/svelte", "@types/node", "eslint", "eslint-plugin-svelte",
    "globals", "jsdom", "prettier", "prettier-plugin-svelte", "svelte",
    "svelte-check", "typescript", "typescript-eslint", "vite", "vitest"
]

crates = [
    "anyhow", "async-trait", "chrono", "clap", "crossterm", "dirs", "futures",
    "globset", "ignore", "ratatui", "regex", "rmcp", "reqwest", "serde",
    "serde_json", "zeroize", "similar", "tempfile", "thiserror", "tokio",
    "tokio-util", "toml", "toml_edit", "tracing", "tracing-subscriber", "uuid"
]

print("--- NPM Updates ---")
for pkg in npm_packages:
    ver = get_npm_version(pkg)
    if ver:
        print(f"{pkg}: {ver}")

print("\n--- Crate Updates ---")
for crate in crates:
    ver = get_crate_version(crate)
    if ver:
        print(f"{crate}: {ver}")
