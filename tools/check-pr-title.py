#!/usr/bin/env python3
"""Validate the squash-merge title consumed by Release Please."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SUPPORTED_TYPES = (
    "feat",
    "fix",
    "perf",
    "deps",
    "refactor",
    "docs",
    "test",
    "build",
    "ci",
    "chore",
    "revert",
)
CHANGELOG_SECTIONS = json.loads(
    (ROOT / "release-please-config.json").read_text(encoding="utf-8")
)["changelog-sections"]
VISIBLE_TYPES = frozenset(
    section["type"]
    for section in CHANGELOG_SECTIONS
    if not section.get("hidden", False) and section["type"] in SUPPORTED_TYPES
)
INTERNAL_SCOPES = frozenset(
    section["type"]
    for section in CHANGELOG_SECTIONS
    if section.get("hidden", False) and section["type"] in SUPPORTED_TYPES
)
TITLE_PATTERN = re.compile(
    rf"^(?P<type>{'|'.join(SUPPORTED_TYPES)})"
    r"(?:\((?P<scope>[a-z0-9][a-z0-9._/-]*)\))?!?: \S(?:.*\S)?$"
)
OVERRIDE_START = "BEGIN_COMMIT_OVERRIDE"
OVERRIDE_END = "END_COMMIT_OVERRIDE"


def is_valid(title: str) -> bool:
    match = TITLE_PATTERN.fullmatch(title)
    return bool(
        match
        and not (
            match.group("type") in VISIBLE_TYPES
            and match.group("scope") in INTERNAL_SCOPES
        )
    )


def metadata_errors(title: str, body: str) -> list[str]:
    errors: list[str] = []
    if not is_valid(title):
        errors.append(f"invalid pull-request title: {title}")

    lines = body.splitlines()
    starts = [index for index, line in enumerate(lines) if line.strip() == OVERRIDE_START]
    ends = [index for index, line in enumerate(lines) if line.strip() == OVERRIDE_END]
    if len(starts) != len(ends) or len(starts) > 1:
        errors.append("commit override markers must form at most one complete block")
        return errors

    if starts:
        start, end = starts[0], ends[0]
        if end <= start:
            errors.append("commit override end marker must follow its start marker")
            return errors
        entries = [line.strip() for line in lines[start + 1 : end] if line.strip()]
        if not entries:
            errors.append("commit override block must contain at least one entry")
        for entry in entries:
            if not is_valid(entry):
                errors.append(f"invalid commit override entry: {entry}")
        duplicates = sorted({entry for entry in entries if entries.count(entry) > 1})
        errors.extend(f"duplicate commit override entry: {entry}" for entry in duplicates)
    else:
        conventional_body_lines = [
            line.strip() for line in lines if TITLE_PATTERN.fullmatch(line.strip())
        ]
        errors.extend(
            f"Conventional Commit line requires a commit override block: {entry}"
            for entry in conventional_body_lines
        )
    return errors


def self_test() -> None:
    valid = (
        "feat: add idle shutdown",
        "fix(menu): retain native status images",
        "perf(runtime)!: replace polling contract",
        "deps: update serial transport dependencies",
        "ci: repair release automation",
        "chore(main): release 1.1.0",
        "revert: restore the previous mapping",
    )
    invalid = (
        "Add idle shutdown",
        "feature: add idle shutdown",
        "fix(Menu): uppercase scopes are not allowed",
        "feat(ci): internal automation is not a user-visible feature",
        "fix(build): internal packaging changes must use the build type",
        "fix: ",
        "fix no colon",
        " fix: leading whitespace",
        "fix: trailing whitespace ",
    )

    failures = [title for title in valid if not is_valid(title)]
    failures.extend(title for title in invalid if is_valid(title))
    if failures:
        raise AssertionError(f"unexpected title-validation results: {failures!r}")
    assert metadata_errors("feat(menu): add profiles", "ordinary description") == []
    assert metadata_errors(
        "feat(menu): add profiles",
        "BEGIN_COMMIT_OVERRIDE\nfeat(menu): add profiles\n\nfix(runtime): recover safely\nEND_COMMIT_OVERRIDE",
    ) == []
    assert metadata_errors(
        "feat: log status changes",
        "feat: log status changes\n\nDetails follow.",
    ) == [
        "Conventional Commit line requires a commit override block: feat: log status changes"
    ]
    assert metadata_errors(
        "feat(menu): add profiles",
        "BEGIN_COMMIT_OVERRIDE\nfix(runtime): recover safely\nfix(runtime): recover safely\nEND_COMMIT_OVERRIDE",
    ) == ["duplicate commit override entry: fix(runtime): recover safely"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("title", nargs="?", help="pull-request title to validate")
    parser.add_argument("--body", default="", help="pull-request description to validate")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()

    if args.title is None:
        if args.self_test:
            return 0
        parser.error("a pull-request title is required")

    errors = metadata_errors(args.title, args.body)
    if not errors:
        return 0

    for error in errors:
        print(error, file=sys.stderr)
    print("Expected release metadata to use valid Conventional Commit entries.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
