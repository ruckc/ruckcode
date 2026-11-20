#!/bin/bash

package_json="app/package.json"

# Extract dependencies and devDependencies
deps=$(jq -r '.dependencies, .devDependencies | to_entries[] | "\(.key) \(.value)"' "$package_json" 2>/dev/null)

if [ -z "$deps" ]; then
    echo "No dependencies found or error parsing package.json"
    exit 1
fi

echo "package version_range"

while read -r name version_range; do
    # Remove version range characters like ^ and ~
    version=$(echo "$version_range" | sed 's/[^0-9.]*//g')
    
    # Fetch latest version from npm registry
    latest=$(curl -s "https://registry.npmjs.org/$name/latest" | jq -r '.version' 2>/dev/null)
    
    if [ -n "$latest" ] && [ "$latest" != "$version" ]; then
        # Check if it's a minor or patch update (simple check)
        # We don't want to do major updates unless they are security fixes (not handled here)
        # For now, just report all updates
        echo "$name $version_range -> $latest"
    fi
done <<< "$deps"
