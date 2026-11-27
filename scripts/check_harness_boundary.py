#!/usr/bin/env python3
"""Fail if any `ruck-harness-*` package depends on a `ruckcode-*` package.

The harness crates are product-neutral: they must never depend on the ruckcode agent or its
front-ends, through normal, build or dev dependencies. See docs/ARCHITECTURE.md.

Uses `cargo metadata --no-deps`, which lists each workspace package's direct dependencies (all
kinds) without resolving or downloading anything, so it is fast and works offline.
Python standard library only.
"""
import json
import subprocess
import sys

HARNESS_PREFIX = "ruck-harness-"
PRODUCT_PREFIX = "ruckcode-"


def violations(metadata):
    """Return sorted (harness package, product package, dependency kind) triples."""
    found = []
    for pkg in metadata.get("packages", []):
        if not pkg["name"].startswith(HARNESS_PREFIX):
            continue
        for dep in pkg.get("dependencies", []):
            if dep["name"].startswith(PRODUCT_PREFIX):
                found.append((pkg["name"], dep["name"], dep.get("kind") or "normal"))
    return sorted(found)


def main(argv):
    extra = argv[1:]  # optional extra cargo args, e.g. --manifest-path
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", *extra],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    bad = violations(json.loads(out))
    for harness, product, kind in bad:
        print(f"error: {harness} depends on {product} ({kind} dependency)", file=sys.stderr)
    if bad:
        print("the ruck-harness-* crates must never depend on ruckcode-* crates", file=sys.stderr)
        return 1
    print("harness boundary ok: no ruck-harness-* crate depends on a ruckcode-* crate")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
