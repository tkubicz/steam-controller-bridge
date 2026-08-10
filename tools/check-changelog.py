#!/usr/bin/env python3
"""Validate the newest generated release-note section."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CHANGELOG_SECTIONS = json.loads(
    (ROOT / "release-please-config.json").read_text(encoding="utf-8")
)["changelog-sections"]
VISIBLE_SECTIONS = frozenset(
    section["section"] for section in CHANGELOG_SECTIONS if not section.get("hidden", False)
)
INTERNAL_SCOPES = frozenset(
    section["type"] for section in CHANGELOG_SECTIONS if section.get("hidden", False)
)


def newest_release(markdown: str) -> tuple[str, list[str]]:
    release: str | None = None
    lines: list[str] = []
    for line in markdown.splitlines():
        if line.startswith("## ["):
            if release is not None:
                break
            release = line.split("]", 1)[0][4:]
        elif release is not None:
            lines.append(line)
    return release or "unversioned", lines


def invalid_entries(markdown: str) -> list[str]:
    release, lines = newest_release(markdown)
    seen: set[str] = set()
    section = ""
    invalid: list[str] = []
    for line in lines:
        if line.startswith("### "):
            section = line[4:]
        elif line.startswith("* "):
            description = line[2:].split(" ([", 1)[0].strip()
            if description in seen:
                invalid.append(f"{release}: duplicate entry: {description}")
            seen.add(description)
            if section in VISIBLE_SECTIONS and description.startswith("**"):
                scope = description[2:].split(":**", 1)[0]
                if scope in INTERNAL_SCOPES:
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
        "## [2.0.0]\n* clean ([abc])\n## [1.0.0]\n* old ([abc])\n* old ([def])"
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
