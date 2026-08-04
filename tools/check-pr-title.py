#!/usr/bin/env python3
"""Validate the squash-merge title consumed by Release Please."""

from __future__ import annotations

import argparse
import re
import sys


TITLE_PATTERN = re.compile(
    r"^(?:feat|fix|perf|deps|refactor|docs|test|build|ci|chore|revert)"
    r"(?:\([a-z0-9][a-z0-9._/-]*\))?!?: \S(?:.*\S)?$"
)


def is_valid(title: str) -> bool:
    return bool(TITLE_PATTERN.fullmatch(title))


def self_test() -> None:
    valid = (
        "feat: add idle shutdown",
        "fix(menu): retain native status images",
        "perf(runtime)!: replace polling contract",
        "deps: update serial transport dependencies",
        "chore(main): release 1.1.0",
        "revert: restore the previous mapping",
    )
    invalid = (
        "Add idle shutdown",
        "feature: add idle shutdown",
        "fix(Menu): uppercase scopes are not allowed",
        "fix: ",
        "fix no colon",
        " fix: leading whitespace",
        "fix: trailing whitespace ",
    )

    failures = [title for title in valid if not is_valid(title)]
    failures.extend(title for title in invalid if is_valid(title))
    if failures:
        raise AssertionError(f"unexpected title-validation results: {failures!r}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("title", nargs="?", help="pull-request title to validate")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()

    if args.title is None:
        if args.self_test:
            return 0
        parser.error("a pull-request title is required")

    if is_valid(args.title):
        return 0

    print(
        "Pull-request title is not a supported Conventional Commit.\n"
        "Expected: type(scope)!: summary, where scope and ! are optional.\n"
        "Allowed types: feat, fix, perf, deps, refactor, docs, test, build, "
        "ci, chore, revert.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
