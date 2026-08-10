#!/usr/bin/env python3
"""Reject repeated release-note bullets inside one generated release."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def duplicate_entries(markdown: str) -> list[str]:
    release = "unversioned"
    seen: set[str] = set()
    duplicates: list[str] = []
    for line in markdown.splitlines():
        if line.startswith("## ["):
            release = line.split("]", 1)[0][4:]
            seen.clear()
        elif line.startswith("* "):
            description = line[2:].split(" ([", 1)[0].strip()
            if description in seen:
                duplicates.append(f"{release}: {description}")
            seen.add(description)
    return duplicates


def self_test() -> None:
    assert duplicate_entries("## [1.0.0]\n\n* one ([abc])\n* two ([def])\n") == []
    assert duplicate_entries("## [1.0.0]\n\n* one ([abc])\n* one ([#1]) ([abc])\n") == [
        "1.0.0: one"
    ]
    assert duplicate_entries("## [2.0.0]\n* one ([abc])\n## [1.0.0]\n* one ([abc])") == []


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    root = Path(__file__).resolve().parent.parent
    duplicates = duplicate_entries((root / "CHANGELOG.md").read_text(encoding="utf-8"))
    for duplicate in duplicates:
        print(f"duplicate changelog entry: {duplicate}", file=sys.stderr)
    return int(bool(duplicates))


if __name__ == "__main__":
    raise SystemExit(main())
