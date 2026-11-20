#!/bin/bash

cargo_toml="Cargo.toml"

# Extract dependencies and workspace.dependencies
deps=$(grep -E '^[a-z0-9\-_]+ = ' "$cargo_toml" | sed 's/ = .*//' | tr '\n' ' ')

if [ -z "$deps" ]; then
    echo "No dependencies found in Cargo.toml"
    exit 1
fi

echo "package version_range -> latest"

for pkg in $deps; do
    # Get the version from Cargo.toml (this is complex because of workspace.dependencies)
    # For simplicity, let's just try to fetch latest from crates.io
    latest=$(curl -s "https://crates.io/api/v1/crates/$pkg" | jq -r '.crate.max_version' 2>/dev/null)
    
    if [ -n "$latest" ] && [ "$latest" != "null" ]; then
        # We don't know the current version easily without parsing more carefully
        # but we can at least see what's available.
        echo "$pkg -> $latest"
    fi
done
