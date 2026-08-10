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
