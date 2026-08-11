"""Shared Release Please metadata policy for repository validators."""

from __future__ import annotations

import json
import re
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
    if section.get("hidden", False)
)
VISIBLE_SECTIONS = frozenset(
    section["section"]
    for section in CHANGELOG_SECTIONS
    if not section.get("hidden", False)
)
TITLE_PATTERN = re.compile(
    rf"^(?P<type>{'|'.join(SUPPORTED_TYPES)})"
    r"(?:\((?P<scope>[a-z0-9][a-z0-9._/-]*)\))?!?: \S(?:.*\S)?$"
)
OVERRIDE_START = "BEGIN_COMMIT_OVERRIDE"
OVERRIDE_END = "END_COMMIT_OVERRIDE"


def scope_root(scope: str | None) -> str | None:
    """Return the policy-bearing first component of a compound scope."""
    return re.split(r"[._/-]", scope, maxsplit=1)[0] if scope else None


def is_internal_scope(scope: str | None) -> bool:
    return scope_root(scope) in INTERNAL_SCOPES


def is_valid_title(title: str) -> bool:
    match = TITLE_PATTERN.fullmatch(title)
    return bool(
        match
        and not (
            match.group("type") in VISIBLE_TYPES
            and is_internal_scope(match.group("scope"))
        )
    )


def override_block(body: str) -> tuple[list[str] | None, list[str]]:
    """Extract one ordered override block and reject marker substrings elsewhere."""
    lines = body.splitlines()
    starts = [index for index, line in enumerate(lines) if line.strip() == OVERRIDE_START]
    ends = [index for index, line in enumerate(lines) if line.strip() == OVERRIDE_END]
    start_occurrences = body.count(OVERRIDE_START)
    end_occurrences = body.count(OVERRIDE_END)
    if start_occurrences == 0 and end_occurrences == 0:
        return None, []
    if start_occurrences != len(starts) or end_occurrences != len(ends):
        return None, ["commit override markers must appear only on standalone lines"]
    if len(starts) != 1 or len(ends) != 1:
        return None, ["commit override markers must form exactly one complete block"]
    if ends[0] <= starts[0]:
        return None, ["commit override end marker must follow its start marker"]
    return lines[starts[0] + 1 : ends[0]], []
