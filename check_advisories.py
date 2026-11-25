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
            res_data = json.loads(response.read().decode('utf-8'))
            return res_data.get("vulns", [])
    except Exception:
        return []

# List of (package, old_version, new_version)
packages = [
    ("toml", "1.2.1+spec-1.1.0", "1.2.2+spec-1.1.0"),
    ("toml_edit", "0.25.19+spec-1.1.0", "0.25.21+spec-1.1.0"),
    ("regex", "1.13.1", "1.13.2"),
    ("regex-automata", "0.4.18", "0.4.19"),
    ("regex-syntax", "0.8.11", "0.8.12"),
    ("tauri", "2.12.5", "2.12.7"),
    ("tauri-build", "2.7.4", "2.7.6"),
    ("tauri-codegen", "2.7.4", "2.7.6"),
    ("sha2", "0.10.9", "0.11.1"),
    ("tauri-macros", "2.7.4", "2.7.6"),
    ("tauri-runtime", "2.12.2", "2.12.4"),
    ("tauri-runtime-wry", "2.12.3", "2.12.5"),
    ("tauri-utils", "2.10.2", "2.10.4"),
    ("toml_parser", "1.1.5+spec-1.1.0", "1.1.7+spec-1.1.0"),
    ("uuid", "1.29.2", "1.29.3"),
    ("zeroize", "1.9.1", "1.9.2"),
    ("zip", "9.5.1", "9.8.1"),
]

results = []
for pkg, old_ver, new_ver in packages:
    # Clean version strings (remove +spec-1.1.0 for OSV if necessary, but OSV usually handles it)
    # Actually, let's try with the version as is first.
    old_vulns = check_osv(pkg, old_ver)
    new_vulns = check_osv(pkg, new_ver)
    
    for vuln in old_vulns:
        # Check if this vuln is still present in new_ver
        # A vuln is still present if it's in new_vulns
        is_fixed = False
        for nv in new_vulns:
            if nv.get("id") == vuln.get("id"):
                is_fixed = True
                break
        
        if not is_fixed:
            results.append({
                "package": pkg,
                "old_version": old_ver,
                "new_version": new_ver,
                "advisory_id": vuln.get("id"),
                "status": "not fixed"
            })

    # Also check if new_ver itself has any NEW vulnerabilities (though unlikely)
    for vuln in new_vulns:
        # Check if this vuln was also in old_ver
        was_in_old = False
        for ov in old_vulns:
            if ov.get("id") == vuln.get("id"):
                was_in_old = True
                break
        if not was_in_old:
            results.append({
                "package": pkg,
                "old_version": old_ver,
                "new_version": new_ver,
                "advisory_id": vuln.get("id"),
                "status": "newly introduced"
            })

print(json.dumps(results))
