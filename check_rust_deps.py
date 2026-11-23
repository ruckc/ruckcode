import urllib.request
import json
import sys

def get_latest_version(crate_name):
    try:
        url = f"https://crates.io/api/v1/crates/{crate_name}"
        with urllib.request.urlopen(url) as response:
            data = json.loads(response.read().decode())
            return data['crate']['max_version']
    except Exception as e:
        return None

deps = {
    "agent-client-protocol": "5.0",
    "anyhow": "1",
    "async-trait": "0.1",
    "chrono": "0.4",
    "clap": "4",
    "crossterm": "0.29",
    "dirs": "7",
    "futures": "0.3",
    "globset": "0.4",
    "ignore": "0.4",
    "ratatui": "0.30",
    "regex": "1",
    "rmcp": "3.9",
    "reqwest": "0.13",
    "serde": "1",
    "serde_json": "1",
    "zeroize": "1",
    "similar": "3",
    "tempfile": "3",
    "thiserror": "2",
    "tokio": "1",
    "tokio-util": "0.7",
    "toml": "1.2",
    "toml_edit": "0.25",
    "tracing": "0.1",
    "tracing-subscriber": "0.3",
    "uuid": "1"
}

print(f"{'crate':<25} | {'current':<10} | {'latest':<10} | {'status'}")
print("-" * 60)

for crate, current in deps.items():
    latest = get_latest_version(crate)
    if not latest:
        print(f"{crate:<25} | {current:<10} | {'Error':<10} | Error")
        continue
    
    # Very basic semver check for major/minor
    # We don't want to suggest major updates unless necessary (not in scope for now)
    # But the prompt says: "Every available patch and minor update... Major updates ONLY when needed to fix a known vulnerability"
    
    status = "No update"
    if latest != current:
        # This is a simplification. In reality, we'd use a semver library.
        # For this task, we just check if the latest starts with the same major/minor if possible.
        # But we'll just report it.
        status = "Update available"

    print(f"{crate:<25} | {current:<10} | {latest:<10} | {status}")

