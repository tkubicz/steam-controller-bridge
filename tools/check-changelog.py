#!/usr/bin/env python3
"""Validate the newest generated release-note section."""

from __future__ import annotations

import argparse
import sys

from release_metadata import ROOT, VISIBLE_SECTIONS, is_internal_scope


def invalid_entries(markdown: str) -> list[str]:
    release = "unversioned"
    seen: set[str] = set()
    section = ""
    invalid: list[str] = []
    for line in markdown.splitlines():
        if line.startswith("## ["):
            release = line.split("]", 1)[0][4:]
            seen.clear()
            section = ""
        elif line.startswith("### "):
            section = line[4:]
        elif line.startswith("* "):
            description = line[2:].split(" ([", 1)[0].strip()
            if description in seen:
                invalid.append(f"{release}: duplicate entry: {description}")
            seen.add(description)
            if section in VISIBLE_SECTIONS and description.startswith("**"):
                scope = description[2:].split(":**", 1)[0]
                if is_internal_scope(scope):
                    invalid.append(
                        f"{release}: internal scope {scope!r} appears under {section}"
                    )
    return invalid


def self_test() -> None:
    assert invalid_entries("## [1.0.0]\n\n* one ([abc])\n* two ([def])\n") == []
    assert invalid_entries("## [1.0.0]\n\n* one ([abc])\n* one ([#1]) ([abc])\n") == [
        "1.0.0: duplicate entry: one"
    ]
    assert invalid_entries(
        "## [2.0.0]\n### Features\n* **ci:** internal ([abc])\n"
    ) == ["2.0.0: internal scope 'ci' appears under Features"]
    assert invalid_entries(
        "## [2.0.0]\n### Bug Fixes\n* **build/macos:** internal ([abc])\n"
    ) == ["2.0.0: internal scope 'build/macos' appears under Bug Fixes"]
    assert invalid_entries(
        "## [2.0.0]\n* clean ([abc])\n## [1.0.0]\n* old ([abc])\n* old ([def])"
    ) == ["1.0.0: duplicate entry: old"]
    assert invalid_entries(
        "## [2.0.0]\n* same ([abc])\n## [1.0.0]\n* same ([def])"
    ) == []


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    invalid = invalid_entries((ROOT / "CHANGELOG.md").read_text(encoding="utf-8"))
    for entry in invalid:
        print(f"invalid changelog entry: {entry}", file=sys.stderr)
    return int(bool(invalid))


if __name__ == "__main__":
    raise SystemExit(main())
