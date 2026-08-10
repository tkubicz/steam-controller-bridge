#!/usr/bin/env python3
"""Validate the squash-merge title consumed by Release Please."""

from __future__ import annotations

import argparse
import re
import sys

from release_metadata import TITLE_PATTERN, is_valid_title


OVERRIDE_START = "BEGIN_COMMIT_OVERRIDE"
OVERRIDE_END = "END_COMMIT_OVERRIDE"
BREAKING_FOOTER = re.compile(r"BREAKING(?: |-)CHANGE: \S(?:.*\S)?$")


def metadata_errors(title: str, body: str) -> list[str]:
    errors: list[str] = []
    if not is_valid_title(title):
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
        override_lines = [line.strip() for line in lines[start + 1 : end] if line.strip()]
        if not override_lines:
            errors.append("commit override block must contain at least one entry")
        entries: list[str] = []
        for line in override_lines:
            if TITLE_PATTERN.fullmatch(line):
                entries.append(line)
                if not is_valid_title(line):
                    errors.append(f"invalid commit override entry: {line}")
            elif entries and BREAKING_FOOTER.fullmatch(line):
                continue
            else:
                errors.append(f"invalid commit override content: {line}")
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
        "feat(style): internal formatting is not a user-visible feature",
        "feat(ci/release): compound internal scopes remain internal",
        "fix(build.macos): compound build scopes remain internal",
        "perf(test_helpers): compound test scopes remain internal",
        "fix(build): internal packaging changes must use the build type",
        "fix: ",
        "fix no colon",
        " fix: leading whitespace",
        "fix: trailing whitespace ",
    )

    failures = [title for title in valid if not is_valid_title(title)]
    failures.extend(title for title in invalid if is_valid_title(title))
    if failures:
        raise AssertionError(f"unexpected title-validation results: {failures!r}")
    assert metadata_errors("feat(menu): add profiles", "ordinary description") == []
    assert metadata_errors(
        "feat(menu): add profiles",
        "BEGIN_COMMIT_OVERRIDE\nfeat(menu): add profiles\n\nfix(runtime): recover safely\nEND_COMMIT_OVERRIDE",
    ) == []
    assert metadata_errors(
        "feat(menu): replace profile storage",
        "BEGIN_COMMIT_OVERRIDE\nfeat(menu): replace profile storage\nBREAKING CHANGE: existing profiles must be migrated\nEND_COMMIT_OVERRIDE",
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
    assert metadata_errors(
        "feat(menu): add profiles",
        "BEGIN_COMMIT_OVERRIDE\nfeat(menu): add profiles\nunsupported body\nEND_COMMIT_OVERRIDE",
    ) == ["invalid commit override content: unsupported body"]


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
