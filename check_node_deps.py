import urllib.request
import json
import sys

def get_latest_version(package_name):
    try:
        url = f"https://registry.npmjs.org/{package_name}/latest"
        with urllib.request.urlopen(url) as response:
            data = json.loads(response.read().decode())
            return data['version']
    except Exception as e:
        return None

def read_pkg_json():
    with open('app/package.json', 'r') as f:
        return json.load(f)

pkg_json = read_pkg_json()
deps_to_check = pkg_json.get('dependencies', {})
dev_deps_to_check = pkg_json.get('devDependencies', {})

all_deps = {**deps_to_check, **dev_deps_to_check}

print(f"{'package':<30} | {'current':<10} | {'latest':<10} | {'status'}")
print("-" * 70)

for pkg, current_spec in all_deps.items():
    # Clean spec (remove ^, ~, etc.)
    current = current_spec.lstrip('^~>=< ')
    latest = get_latest_version(pkg)
    
    if not latest:
        print(f"{pkg:<30} | {current:<10} | {'Error':<10} | Error")
        continue
    
    status = "No update"
    if latest != current:
        # We are only doing minor/patch updates. 
        # For simplicity, we'll just check if the major version is the same.
        current_major = current.split('.')[0]
        latest_major = latest.split('.')[0]
        if current_major == latest_major:
            status = "Update available"
        else:
            status = "Major update available (skip)"

    print(f"{pkg:<30} | {current:<10} | {latest:<10} | {status}")
