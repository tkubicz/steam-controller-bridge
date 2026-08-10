#!/usr/bin/env python3
"""Validate the single-product version invariant."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path


RELEASE_VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")


def validate(root: Path) -> tuple[str | None, list[str]]:
    errors: list[str] = []
    root_manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
    version_match = re.search(
        r"(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*\"([^\"]+)\"",
        root_manifest,
    )
    expected = version_match.group(1) if version_match else None
    if expected is None or RELEASE_VERSION.fullmatch(expected) is None:
        errors.append(f"workspace.package.version is not a release SemVer: {expected!r}")
        return None, errors

    version_file = (root / "version.txt").read_text(encoding="utf-8").strip()
    if version_file != expected:
        errors.append(f"version.txt is {version_file!r}; expected {expected!r}")

    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    workspace_members = set(metadata["workspace_members"])
    for package in metadata["packages"]:
        if package["id"] in workspace_members and package["version"] != expected:
            relative_manifest = Path(package["manifest_path"]).relative_to(root)
            errors.append(
                f"{relative_manifest.parent} ({package['name']}) is {package['version']!r}; expected {expected!r}"
            )
    return expected, errors


def self_test() -> None:
    accepted = ["0.0.0", "1.5.0", "10.20.300"]
    rejected = ["1.2", "1.2.3oops", "01.2.3", "1.02.3", "v1.2.3", "1.2.3-rc.1"]
    assert all(RELEASE_VERSION.fullmatch(value) for value in accepted)
    assert all(RELEASE_VERSION.fullmatch(value) is None for value in rejected)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--print-version", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    root = Path(__file__).resolve().parent.parent
    version, errors = validate(root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    if arguments.print_version:
        print(version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
