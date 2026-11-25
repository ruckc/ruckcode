import json
import subprocess

# Format: (name, version, ecosystem)
packages = [
    ("@agentclientnamespace/sdk", "2.9.1", "npm"), # Wait, I should use the correct name from the diff
    ("@agentclientprotocol/sdk", "2.9.1", "npm"),
    ("@ruckc/truss-chat", "1.44.0", "npm"),
    ("@ruckc/truss-code", "1.44.0", "npm"),
    ("@ruckc/truss-markdown", "1.44.0", "npm"),
    ("@ruckc/truss-ui", "1.44.0", "npm"),
    ("@tauri-apps/api", "2.12.5", "npm"),
    ("@sveltejs/kit", "3.4.0", "npm"),
    ("@tauri-apps/cli", "2.12.6", "npm"),
    ("@testing-library/jest-dom", "8.0.0", "npm"),
    ("prettier-plugin-svelte", "4.2.0", "npm"),
    ("svelte", "5.59.1", "npm"),
    ("typescript-eslint", "8.73.0", "npm"),
    ("vitest", "5.1.1", "npm"),
    ("esrap", "2.4.2", "npm"),
    ("ignore", "7.0.16", "npm"),
    ("minimatch", "11.0.9", "npm"),
    ("tinybench", "6.2.4", "npm"),
    ("syn", "3.0.9", "Cargo"),
    ("form_urlencoded", "1.2.2", "Cargo"),
    ("url", "2.5.8", "Cargo"),
    ("cookie", "0.18.2", "Cargo"),
]

def check_osv(package, version, ecosystem):
    query = {
        "version": version,
        "package": {"name": package, "ecosystem": ecosystem}
    }
    
    try:
        process = subprocess.Popen(
            ['curl', '-s', '-X', 'POST', '-H', 'Content-Type: application/json',
             '-d', json.dumps(query), 'https://api.osv.dev/v1/query'],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
        stdout, stderr = process.communicate()
        if stderr:
            return None
        return json.dumps(json.loads(stdout))
    except Exception as e:
        return None

results = []
for pkg, ver, eco in packages:
    res_str = check_osv(pkg, ver, eco)
    if res_str:
        res = json.loads(res_str)
        if res.get('vulns'):
            results.append((pkg, ver, eco, res['vulns']))

print(json.dumps(results))
