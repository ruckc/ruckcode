import urllib.request
import json
import sys

def check_osv(package, version):
    url = "https://api.osv.dev/v1/query"
    payload = {
        "version": version,
        "package": {"name": package, "ecosystem": "crates.io"}
    }
    data = json.dumps(payload).encode('utf-8')
    req = urllib.request.Request(url, data=data, headers={'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req) as response:
            res_data = json.loads(response.read().decode('').decode('utf-8'))
            return res_data.get("vulns", [])
    except Exception:
        return []

# List of (package, old_version)
packages = [
    ("toml", "1.2.1"),
    ("toml_edit", "0.25.19"),
    ("regex", "1.13.1"),
    ("regex-automata", "0.4.18"),
    ("regex-syntax", "0.8.11"),
    ("tauri", "2.12.5"),
    ("tauri-build", "2.7.4"),
    ("tauri-codegen", "2.7.4"),
    ("sha2", "0.10.9"),
    ("tauri-macros", "2.7.4"),
    ("tauri-runtime", "2.12.2"),
    ("tauri-runtime-wry", "2.12.3"),
    ("tauri-utils", "2.10.2"),
    ("toml_parser", "1.1.5"),
    ("uuid", "1.29.2"),
    ("zeroize", "1.9.1"),
    ("zip", "9.5.1"),
]

results = []
for pkg, ver in packages:
    vulns = check_osv(pkg, ver)
    if vulns:
        results.append((pkg, ver, vulns))

print(json.dumps(exports := [ [p, v, [vuln['id'] for vuln in vs]] for p, v, vs in results ]))
