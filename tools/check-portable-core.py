#!/usr/bin/env python3
"""Reject platform implementation details in the portable core."""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path, PurePosixPath


PORTABLE_CRATES = (
    "bridge-core",
    "bridge-output",
    "bridge-protocol",
    "bridge-runtime",
    "controller-mapper",
    "desktop-bindings",
    "gamepad-state",
    "profile-picker",
    "recording",
    "steam-controller-discovery",
    "steam-controller-protocol",
)

FACADE_ROOTS = {
    PurePosixPath("crates/bridge-output/src/serial_discovery.rs"),
}
FACADE_DIRECTORIES = {
    PurePosixPath("crates/bridge-output/src/serial_discovery"),
}

PLATFORM_CFG = re.compile(
    r"\b(?:target_(?:arch|env|family|os|vendor)|unix|windows)\b"
)
PLATFORM_DATA_PATH = re.compile(
    r"(?:"
    r"(?<![A-Za-z0-9_:/.-])(?i:file):/+(?:(?i:localhost)/+)?(?:"
    r"(?i:[A-Za-z]:/+(?:ProgramData|Users))|"
    r"Users|home|System/Library|Library/(?:Application Support|Caches|Logs)|"
    r"Applications|etc|var|tmp|usr/(?:local/)?share|dev(?:/|\b)|"
    r"run/(?:media|user)\b)\b|"
    r"(?<![A-Za-z0-9.:/])/(?:Users|home|System/Library|Applications|etc|var|tmp|"
    r"usr/(?:local/)?share)\b|"
    r"(?<![A-Za-z0-9.:/])/(?:dev(?:/|\b)|run/(?:media|user)\b)|"
    r"(?<![A-Za-z0-9.:/])~/Library/(?:Application Support|Caches|Logs)\b|"
    r"(?<![A-Za-z0-9.:/~])/Library/(?:Application Support|Caches|Logs)\b|"
    r"(?<![A-Za-z0-9.:/])(?:\.\.?/)+Library/(?:Application Support|Caches|Logs)\b|"
    r"(?<![/A-Za-z0-9])Library/(?:Application Support|Caches|Logs)\b|"
    r"(?<![A-Za-z0-9.:/])(?i:[A-Za-z]:[\\/]+(?:ProgramData|Users))\b|"
    r"(?i:\\+AppData)\b|"
    r"(?<![A-Za-z0-9_])(?:~|\$HOME|\$\{HOME\})?/?\."
    r"(?:(?:cache|config)/|local/(?:share|state)\b)|"
    r"[\"'](?:APPDATA|HOME|LOCALAPPDATA|PROGRAMDATA|USERPROFILE|XDG_CONFIG_HOME|"
    r"XDG_DATA_HOME|XDG_STATE_HOME|XDG_CACHE_HOME|XDG_RUNTIME_DIR)[\"']|"
    r"\$(?:APPDATA|HOME|LOCALAPPDATA|PROGRAMDATA|USERPROFILE|XDG_CONFIG_HOME|"
    r"XDG_DATA_HOME|XDG_STATE_HOME|XDG_CACHE_HOME|XDG_RUNTIME_DIR)\b|"
    r"\$\{(?:APPDATA|HOME|LOCALAPPDATA|PROGRAMDATA|USERPROFILE|XDG_CONFIG_HOME|"
    r"XDG_DATA_HOME|XDG_STATE_HOME|XDG_CACHE_HOME|XDG_RUNTIME_DIR)\}|"
    r"%(?:APPDATA|HOME|LOCALAPPDATA|PROGRAMDATA|USERPROFILE|XDG_CONFIG_HOME|"
    r"XDG_DATA_HOME|XDG_STATE_HOME|XDG_CACHE_HOME|XDG_RUNTIME_DIR)%"
    r")"
)
NATIVE_PACKAGES = frozenset(
    {
        "ashpd",
        "block2",
        "cocoa",
        "core-foundation",
        "core-graphics",
        "dispatch2",
        "enigo",
        "evdev",
        "gio",
        "glib",
        "gtk",
        "gtk4",
        "hidapi",
        "libc",
        "libei",
        "libudev",
        "nix",
        "objc2",
        "reis",
        "udev",
        "uinput",
        "wayland-client",
        "winapi",
        "windows",
        "windows-core",
        "windows-sys",
        "windows-targets",
        "x11rb",
        "zbus",
    }
)
NATIVE_MARKERS = tuple(
    package.replace("-", "_") + "::" for package in sorted(NATIVE_PACKAGES)
) + ("libudev", "objc2", "std::os::")
RUST_CHARACTER_LITERAL = re.compile(
    r"(?:b)?'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\}|.)|[^\\'\n])'"
)
RUST_RAW_STRING = re.compile(r'(?:b|c)?r(#{0,255})"')
CFG_EXPRESSION_START = re.compile(r"\bcfg(?:_attr)?!?\s*\(")
BUILD_TARGET_ENV = re.compile(
    r"[\"'](?:CARGO_CFG_[A-Z0-9_]+|HOST|TARGET)[\"']"
)
RUST_CALL_OPEN = r"(?:\s*::\s*<[^;(){}]{1,200}>)?\s*\("
BUILD_TARGET_ACCESS = re.compile(
    r"\bstd::env::(?:var|var_os)" + RUST_CALL_OPEN + r"|"
    r"\b(?:env|option_env)!\s*\("
)
BUILD_ENV_ENUMERATION = re.compile(r"\bstd::env::vars(?:_os)?\s*\(\s*\)")
TARGET_CONST_MODULE = re.compile(r"\bstd::env::consts\b")


def is_native_package(package: str) -> bool:
    return package in NATIVE_PACKAGES or package.startswith("objc2-")


def is_portable_file(path: PurePosixPath) -> bool:
    """Return whether a source or manifest belongs to the portable allowlist."""
    return (
        len(path.parts) >= 3
        and path.parts[0] == "crates"
        and path.parts[1] in PORTABLE_CRATES
        and (path.name == "Cargo.toml" or path.suffix == ".rs")
    )


def is_backend_file(path: PurePosixPath) -> bool:
    return any(directory in path.parents for directory in FACADE_DIRECTORIES)


def scan_rust(text: str) -> tuple[str, str]:
    comments_masked = list(text)
    noncode_masked = list(text)

    def erase(masked: list[str], start: int, end: int) -> None:
        for position in range(start, end):
            if masked[position] != "\n":
                masked[position] = " "

    def erase_comment(start: int, end: int) -> None:
        erase(comments_masked, start, end)
        erase(noncode_masked, start, end)

    index = 0
    while index < len(text):
        if text.startswith("//", index):
            end = text.find("\n", index + 2)
            end = len(text) if end == -1 else end
            erase_comment(index, end)
            index = end
            continue
        if text.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(text) and depth:
                if text.startswith("/*", end):
                    depth += 1
                    end += 2
                elif text.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            erase_comment(index, end)
            index = end
            continue
        character = RUST_CHARACTER_LITERAL.match(text, index)
        if character is not None:
            end = character.end()
            erase(noncode_masked, index, end)
            index = end
            continue
        raw = RUST_RAW_STRING.match(text, index)
        if raw is not None:
            closing = '"' + raw.group(1)
            end = text.find(closing, raw.end())
            end = len(text) if end == -1 else end + len(closing)
            erase(noncode_masked, index, end)
            index = end
            continue
        quote = index
        if text[index] in {"b", "c"} and index + 1 < len(text) and text[index + 1] == '"':
            quote += 1
        if text[quote] == '"':
            end = quote + 1
            while end < len(text):
                if text[end] == "\\":
                    end += 2
                elif text[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            erase(noncode_masked, index, end)
            index = end
            continue
        index += 1
    return "".join(comments_masked), "".join(noncode_masked)


def mask_toml_comments(text: str) -> str:
    masked = list(text)
    index = 0
    while index < len(text):
        if text[index] == "#":
            end = text.find("\n", index + 1)
            end = len(text) if end == -1 else end
            for position in range(index, end):
                masked[position] = " "
            index = end
            continue
        if text[index] not in {"\"", "'"}:
            index += 1
            continue
        quote = text[index]
        delimiter = quote * (3 if text.startswith(quote * 3, index) else 1)
        end = index + len(delimiter)
        while end < len(text):
            if quote == '"' and text[end] == "\\":
                end += 2
            elif text.startswith(delimiter, end):
                end += len(delimiter)
                break
            else:
                end += 1
        index = end
    return "".join(masked)


def decode_toml_basic_string(value: str) -> str | None:
    decoded: list[str] = []
    escapes = {"b": "\b", "t": "\t", "n": "\n", "f": "\f", "r": "\r", '"': '"', "\\": "\\"}
    index = 0
    while index < len(value):
        if value[index] != "\\":
            decoded.append(value[index])
            index += 1
            continue
        index += 1
        if index == len(value):
            return None
        escape = value[index]
        if escape in escapes:
            decoded.append(escapes[escape])
            index += 1
            continue
        if escape not in {"u", "U"}:
            return None
        digits = 4 if escape == "u" else 8
        codepoint = value[index + 1 : index + 1 + digits]
        if len(codepoint) != digits or not all(
            character in "0123456789abcdefABCDEF" for character in codepoint
        ):
            return None
        try:
            decoded.append(chr(int(codepoint, 16)))
        except ValueError:
            return None
        index += digits + 1
    return "".join(decoded)


def toml_key_path(text: str) -> tuple[str, ...] | None:
    parts: list[str] = []
    index = 0
    while True:
        while index < len(text) and text[index].isspace():
            index += 1
        if index == len(text):
            return tuple(parts) if parts else None
        if text[index] in {"\"", "'"}:
            quote = text[index]
            index += 1
            start = index
            while index < len(text) and text[index] != quote:
                if quote == '"' and text[index] == "\\":
                    index += 2
                else:
                    index += 1
            if index == len(text):
                return None
            raw = text[start:index]
            decoded = raw if quote == "'" else decode_toml_basic_string(raw)
            if decoded is None:
                return None
            parts.append(decoded)
            index += 1
        else:
            match = re.match(r"[A-Za-z0-9_-]+", text[index:])
            if match is None:
                return None
            parts.append(match.group())
            index += len(match.group())
        while index < len(text) and text[index].isspace():
            index += 1
        if index == len(text):
            return tuple(parts)
        if text[index] != ".":
            return None
        index += 1


def toml_assignment(text: str) -> tuple[tuple[str, ...], str] | None:
    quote: str | None = None
    index = 0
    while index < len(text):
        character = text[index]
        if quote is not None:
            if quote == '"' and character == "\\":
                index += 2
                continue
            if character == quote:
                quote = None
        elif character in {"\"", "'"}:
            quote = character
        elif character == "=":
            key = toml_key_path(text[:index])
            return None if key is None else (key, text[index + 1 :])
        index += 1
    return None


def toml_brace_balance(text: str) -> int:
    balance = 0
    quote: str | None = None
    index = 0
    while index < len(text):
        character = text[index]
        if quote is not None:
            if quote == '"' and character == "\\":
                index += 2
                continue
            if character == quote:
                quote = None
        elif character in {"\"", "'"}:
            quote = character
        elif character == "{":
            balance += 1
        elif character == "}":
            balance -= 1
        index += 1
    return balance


def toml_multiline_opening(text: str) -> str | None:
    index = 0
    while index < len(text):
        if text.startswith('"""', index) or text.startswith("'''", index):
            delimiter = text[index : index + 3]
            closing = toml_multiline_closing(text, delimiter, index + 3)
            if closing == -1:
                return delimiter
            index = closing + 3
            continue
        if text[index] not in {'"', "'"}:
            index += 1
            continue
        quote = text[index]
        index += 1
        while index < len(text):
            if quote == '"' and text[index] == "\\":
                index += 2
            elif text[index] == quote:
                index += 1
                break
            else:
                index += 1
    return None


def toml_multiline_closing(text: str, delimiter: str, start: int = 0) -> int:
    index = text.find(delimiter, start)
    while index != -1 and delimiter == '"""':
        backslashes = 0
        before = index - 1
        while before >= 0 and text[before] == "\\":
            backslashes += 1
            before -= 1
        if backslashes % 2 == 0:
            break
        index = text.find(delimiter, index + 3)
    return index


def toml_structure(
    text: str,
    source_lines: list[str] | None = None,
    scanned_lines: list[str] | None = None,
) -> tuple[
    list[tuple[int, str, tuple[str, ...]]],
    list[tuple[int, str, tuple[str, ...], tuple[str, ...], str]],
]:
    if source_lines is None:
        source_lines = text.splitlines()
    if scanned_lines is None:
        scanned_lines = mask_toml_comments(text).splitlines()
    headers: list[tuple[int, str, tuple[str, ...]]] = []
    statements: list[tuple[int, str, tuple[str, ...], tuple[str, ...], str]] = []
    table: tuple[str, ...] = ()
    index = 0
    while index < len(scanned_lines):
        multiline_delimiter = toml_multiline_opening(scanned_lines[index])
        if multiline_delimiter is not None:
            entry = scanned_lines[index]
            end = index + 1
            while end < len(scanned_lines):
                entry += "\n" + scanned_lines[end]
                if toml_multiline_closing(scanned_lines[end], multiline_delimiter) != -1:
                    break
                end += 1
            assignment = toml_assignment(entry)
            if assignment is not None:
                key, value = assignment
                statements.append(
                    (index + 1, source_lines[index].strip(), table, key, value)
                )
            index = end + 1
            continue
        stripped = scanned_lines[index].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            inner = stripped[2:-2] if stripped.startswith("[[") else stripped[1:-1]
            parsed = toml_key_path(inner)
            if parsed is not None:
                table = parsed
                headers.append((index + 1, source_lines[index].strip(), table))
            index += 1
            continue
        entry = scanned_lines[index]
        balance = toml_brace_balance(entry)
        end = index
        while balance > 0 and end + 1 < len(scanned_lines):
            end += 1
            entry += "\n" + scanned_lines[end]
            balance += toml_brace_balance(scanned_lines[end])
        assignment = toml_assignment(entry)
        if assignment is not None:
            key, value = assignment
            statements.append((index + 1, source_lines[index].strip(), table, key, value))
        index = end + 1
    return headers, statements


def inline_table_entries(value: str) -> list[tuple[tuple[str, ...], str]]:
    value = value.strip()
    if not (value.startswith("{") and value.endswith("}")):
        return []
    content = value[1:-1]
    entries: list[tuple[tuple[str, ...], str]] = []
    start = 0
    depth = 0
    quote: str | None = None
    index = 0
    while index <= len(content):
        character = content[index] if index < len(content) else ","
        if quote is not None:
            if quote == '"' and character == "\\":
                index += 2
                continue
            if character == quote:
                quote = None
        elif character in {"\"", "'"}:
            quote = character
        elif character in "{[":
            depth += 1
        elif character in "}]":
            depth -= 1
        elif character == "," and depth == 0:
            assignment = toml_assignment(content[start:index])
            if assignment is not None:
                entries.append(assignment)
            start = index + 1
        index += 1
    return entries


def toml_string(value: str) -> str | None:
    value = value.strip()
    if len(value) < 2 or value[0] not in {"\"", "'"} or value[-1] != value[0]:
        return None
    if value.startswith(value[0] * 3) and value.endswith(value[0] * 3):
        inner = value[3:-3]
        if inner.startswith("\r\n"):
            inner = inner[2:]
        elif inner.startswith("\n"):
            inner = inner[1:]
        if value[0] == '"':
            inner = re.sub(r"\\[ \t]*\r?\n[ \t\r\n]*", "", inner)
    else:
        inner = value[1:-1]
    return inner if value[0] == "'" else decode_toml_basic_string(inner)


def cfg_expressions(text: str, code: str) -> list[tuple[int, str, str]]:
    expressions: list[tuple[int, str, str]] = []
    for match in CFG_EXPRESSION_START.finditer(code):
        opening = code.find("(", match.start(), match.end())
        depth = 0
        index = opening
        while index < len(code):
            character = code[index]
            if character == "(":
                depth += 1
            elif character == ")":
                depth -= 1
                if depth == 0:
                    expressions.append(
                        (
                            match.start(),
                            text[match.start() : index + 1],
                            code[match.start() : index + 1],
                        )
                    )
                    break
            index += 1
    return expressions


DEPENDENCY_TABLE_NAMES = frozenset(
    {"dependencies", "dev-dependencies", "build-dependencies"}
)


def dependency_is_native(
    name: str, value: str, native_aliases: frozenset[str] = frozenset()
) -> bool:
    if is_native_package(name) or name in native_aliases:
        return True
    for key, package_value in inline_table_entries(value):
        package = toml_string(package_value) if key == ("package",) else None
        if package is not None and is_native_package(package):
            return True
    return False


def inline_native_dependencies(
    value: str, native_aliases: frozenset[str] = frozenset()
) -> set[str]:
    native: set[str] = set()
    for key, dependency_value in inline_table_entries(value):
        if not key:
            continue
        name = key[0]
        if len(key) == 1 and dependency_is_native(
            name, dependency_value, native_aliases
        ):
            native.add(name)
        elif len(key) >= 2 and key[1] == "package":
            package = toml_string(dependency_value)
            if package is not None and is_native_package(package):
                native.add(name)
    return native


def workspace_dependency_aliases(key: tuple[str, ...], value: str) -> set[str]:
    if not key or key[0] != "dependencies":
        return set()
    if len(key) == 1:
        return inline_native_dependencies(value)
    name = key[1]
    if len(key) == 2 and dependency_is_native(name, value):
        return {name}
    if len(key) >= 3 and key[2] == "package":
        package = toml_string(value)
        return {name} if package is not None and is_native_package(package) else set()
    return set()


def inline_workspace_aliases(value: str) -> set[str]:
    aliases: set[str] = set()
    for key, workspace_value in inline_table_entries(value):
        aliases.update(workspace_dependency_aliases(key, workspace_value))
    return aliases


def workspace_native_aliases(text: str) -> set[str]:
    aliases: set[str] = set()
    _, statements = toml_structure(text)
    for _, _, table, key, value in statements:
        if table[:2] == ("workspace", "dependencies"):
            if len(table) == 2:
                aliases.update(
                    workspace_dependency_aliases(("dependencies", *key), value)
                )
            elif len(table) >= 3:
                name = table[2]
                if is_native_package(name):
                    aliases.add(name)
                elif key and key[0] == "package":
                    package = toml_string(value)
                    if package is not None and is_native_package(package):
                        aliases.add(name)
            continue
        if table == ("workspace",):
            aliases.update(workspace_dependency_aliases(key, value))
            continue
        if table:
            continue
        if key == ("workspace",):
            aliases.update(inline_workspace_aliases(value))
        elif key and key[0] == "workspace":
            aliases.update(workspace_dependency_aliases(key[1:], value))
    return aliases


def build_env_enumeration_keys(
    code: str,
    env_imports: tuple[set[str], set[str], set[str], set[str], set[int]],
) -> set[str]:
    modules, _, enumerators, _, _ = env_imports
    calls = [BUILD_ENV_ENUMERATION.pattern]
    calls.extend(
        rf"\b{re.escape(module)}::vars(?:_os)?\s*\(\s*\)"
        for module in modules
    )
    calls.extend(
        rf"\b{re.escape(enumerator)}\s*\(\s*\)" for enumerator in enumerators
    )
    enumeration = "(?:" + "|".join(calls) + ")"
    bindings = {
        match.group(1)
        for match in re.finditer(
            enumeration + r"[^;]{0,500}?\|\s*\(?\s*"
            r"([A-Za-z_][A-Za-z0-9_]*)",
            code,
            re.DOTALL,
        )
    }
    keys = {binding for binding in bindings}
    keys.update(f"{binding}.0" for binding in bindings)
    for assignment in re.finditer(
        r"\blet\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"
        + enumeration,
        code,
    ):
        iterator = re.escape(assignment.group(1))
        remainder = code[assignment.end() :]
        closure = re.search(
            rf"\b{iterator}\s*\.[^;]{{0,500}}?\|\s*\(?\s*"
            r"([A-Za-z_][A-Za-z0-9_]*)",
            remainder,
            re.DOTALL,
        )
        if closure is not None:
            keys.add(closure.group(1))
            keys.add(f"{closure.group(1)}.0")
        for_loop = re.search(
            rf"\bfor\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*,[^)]*\)\s+in\s+"
            rf"{iterator}\b",
            remainder,
            re.DOTALL,
        )
        if for_loop is not None:
            keys.add(for_loop.group(1))
    keys.update(
        match.group(1)
        for match in re.finditer(
            r"\bfor\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*,[^)]*\)\s+in\s+"
            + enumeration,
            code,
            re.DOTALL,
        )
    )
    return keys


def split_rust_use_items(text: str) -> list[str]:
    items: list[str] = []
    depth = 0
    start = 0
    for index, character in enumerate(text):
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
        elif character == "," and depth == 0:
            items.append(text[start:index])
            start = index + 1
    items.append(text[start:])
    return items


def expand_rust_use_tree(
    tree: str, prefix: tuple[str, ...] = ()
) -> list[tuple[tuple[str, ...], str | None]]:
    tree = tree.strip().removeprefix("::")
    opening = tree.find("{")
    if opening != -1:
        depth = 0
        closing = -1
        for index in range(opening, len(tree)):
            if tree[index] == "{":
                depth += 1
            elif tree[index] == "}":
                depth -= 1
                if depth == 0:
                    closing = index
                    break
        if closing == -1:
            return []
        stem = tree[:opening].strip().removesuffix("::")
        stem_parts = tuple(part for part in stem.split("::") if part)
        expanded: list[tuple[tuple[str, ...], str | None]] = []
        for item in split_rust_use_items(tree[opening + 1 : closing]):
            expanded.extend(expand_rust_use_tree(item, (*prefix, *stem_parts)))
        return expanded

    alias = None
    alias_match = re.search(r"\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*$", tree)
    if alias_match is not None:
        alias = alias_match.group(1)
        tree = tree[: alias_match.start()].strip()
    parts = tuple(part for part in tree.split("::") if part)
    if parts == ("self",):
        return [(prefix, alias)]
    return [((*prefix, *parts), alias)]


def rust_env_imports(
    code: str,
) -> tuple[set[str], set[str], set[str], set[str], set[int]]:
    modules: set[str] = set()
    accessors: set[str] = set()
    enumerators: set[str] = set()
    const_modules: set[str] = set()
    const_imports: set[int] = set()
    statements = list(re.finditer(r"\buse\s+([^;]+);", code, re.DOTALL))
    imports = [
        (statement, path, alias)
        for statement in statements
        for path, alias in expand_rust_use_tree(statement.group(1))
    ]
    aliases: dict[str, tuple[str, ...]] = {"std": ("std",)}
    for _ in range(len(imports) + 1):
        changed = False
        for _, path, alias in imports:
            if not path or alias is None or path[0] not in aliases:
                continue
            canonical = (*aliases[path[0]], *path[1:])
            if canonical in {("std",), ("std", "env"), ("std", "env", "consts")}:
                if aliases.get(alias) != canonical:
                    aliases[alias] = canonical
                    changed = True
        if not changed:
            break
    for alias, path in aliases.items():
        if path == ("std",) and alias != "std":
            modules.add(f"{alias}::env")
        elif path == ("std", "env"):
            modules.add(alias)
        elif path == ("std", "env", "consts"):
            const_modules.add(alias)
    for statement, path, alias in imports:
        if path and path[0] in aliases:
            path = (*aliases[path[0]], *path[1:])
        if path[:2] != ("std", "env"):
            if path == ("std", "*"):
                modules.add("env")
            continue
        if len(path) == 2:
            modules.add(alias or "env")
        elif path[2] in {"var", "var_os"}:
            accessors.add(alias or path[2])
        elif path[2] in {"vars", "vars_os"}:
            enumerators.add(alias or path[2])
        elif path[2] == "*":
            accessors.update({"var", "var_os"})
            enumerators.update({"vars", "vars_os"})
            const_modules.add("consts")
        elif path[2] == "consts":
            if len(path) == 3:
                const_modules.add(alias or "consts")
            const_imports.add(statement.start())
    return modules, accessors, enumerators, const_modules, const_imports


def build_target_bindings(scanned: str, code: str) -> set[str]:
    bindings: set[str] = set()
    for target in BUILD_TARGET_ENV.finditer(scanned):
        before = code[max(0, target.start() - 300) : target.start()]
        declaration = re.search(
            r"\b(?:const|static|let(?:\s+mut)?)\s+"
            r"([A-Za-z_][A-Za-z0-9_]*)\s*(?::[^;=]{0,200})?=\s*$",
            before,
        )
        if declaration is not None:
            bindings.add(declaration.group(1))
    return bindings


def build_target_accesses(
    scanned: str,
    code: str,
    env_imports: tuple[set[str], set[str], set[str], set[str], set[int]],
) -> list[tuple[int, str]]:
    accesses: list[tuple[int, str]] = []
    target_bindings = build_target_bindings(scanned, code)
    starts = list(BUILD_TARGET_ACCESS.finditer(code))
    modules, accessors, _, _, _ = env_imports
    for module in modules:
        starts.extend(
            re.finditer(
                rf"\b{re.escape(module)}::(?:var|var_os)" + RUST_CALL_OPEN,
                code,
            )
        )
    for accessor in accessors:
        starts.extend(
            re.finditer(rf"\b{re.escape(accessor)}" + RUST_CALL_OPEN, code)
        )
    for start in starts:
        end = code.find(")", start.end())
        end = len(code) if end == -1 else end + 1
        target = BUILD_TARGET_ENV.search(scanned, start.end(), end)
        uses_target_binding = any(
            re.search(rf"\b{re.escape(binding)}\b", code[start.end() : end])
            for binding in target_bindings
        )
        if target is not None or uses_target_binding:
            accesses.append((start.start(), scanned[start.start() : end]))
    keys = build_env_enumeration_keys(code, env_imports)
    for target in BUILD_TARGET_ENV.finditer(scanned):
        before = code[max(0, target.start() - 300) : target.start()]
        after = code[target.end() : target.end() + 300]
        for key in keys:
            escaped = re.escape(key)
            key_view = rf"\b{escaped}\b(?:\.as_str\(\))?"
            if (
                re.search(rf"{key_view}\s*(?:==|!=)\s*$", before)
                or re.match(rf"\s*(?:==|!=)\s*{key_view}", after)
                or re.search(rf"\bmatches!\s*\(\s*{key_view}\s*,\s*$", before)
                or re.search(rf"\bmatch\s+{key_view}\s*\{{[^}}]*$", before)
            ):
                accesses.append((target.start(), target.group()))
                break
    for key in keys:
        escaped_key = re.escape(key)
        key_view = rf"\b{escaped_key}\b(?:\.as_str\(\))?"
        for binding in target_bindings:
            escaped_binding = rf"\b{re.escape(binding)}\b"
            comparison = re.search(
                rf"(?:{key_view}\s*(?:==|!=)\s*{escaped_binding}|"
                rf"{escaped_binding}\s*(?:==|!=)\s*{key_view}|"
                rf"\bmatches!\s*\(\s*{key_view}\s*,\s*{escaped_binding})",
                code,
            )
            if comparison is not None:
                accesses.append((comparison.start(), comparison.group()))
                break
    return accesses


def dependency_table_index(table: tuple[str, ...]) -> int | None:
    if table and table[0] in DEPENDENCY_TABLE_NAMES:
        return 0
    if len(table) >= 3 and table[0] == "target" and table[2] in DEPENDENCY_TABLE_NAMES:
        return 2
    return None


def manifest_dependency_is_native(
    table: tuple[str, ...],
    key: tuple[str, ...],
    value: str,
    native_aliases: frozenset[str],
) -> bool:
    dependency_index = dependency_table_index(table)
    if dependency_index is not None:
        dependency_table = table[dependency_index:]
        if len(dependency_table) >= 2:
            name = dependency_table[1]
            if is_native_package(name) or name in native_aliases:
                return False
            return (
                bool(key)
                and key[0] == "package"
                and (package := toml_string(value)) is not None
                and is_native_package(package)
            )
        if not key:
            return False
        name = key[0]
        if len(key) == 1:
            return dependency_is_native(name, value, native_aliases)
        if is_native_package(name) or name in native_aliases:
            return True
        return (
            key[1] == "package"
            and (package := toml_string(value)) is not None
            and is_native_package(package)
        )
    if table or not key or key[0] not in DEPENDENCY_TABLE_NAMES:
        return False
    if len(key) == 1:
        return bool(inline_native_dependencies(value, native_aliases))
    name = key[1]
    if len(key) == 2:
        return dependency_is_native(name, value, native_aliases)
    if is_native_package(name) or name in native_aliases:
        return True
    return (
        key[2] == "package"
        and (package := toml_string(value)) is not None
        and is_native_package(package)
    )


def manifest_policy_errors(
    path: PurePosixPath,
    text: str,
    native_aliases: frozenset[str],
    source_lines: list[str] | None = None,
    scanned_lines: list[str] | None = None,
) -> list[str]:
    errors: list[str] = []
    headers, statements = toml_structure(text, source_lines, scanned_lines)
    for line_number, source, table in headers:
        if table and table[0] == "target":
            errors.append(f"{path}:{line_number}: platform cfg: {source}")
        if (
            (dependency_index := dependency_table_index(table)) is not None
            and len(table) > dependency_index + 1
            and (
                is_native_package(table[dependency_index + 1])
                or table[dependency_index + 1] in native_aliases
            )
        ):
            errors.append(f"{path}:{line_number}: native backend detail: {source}")
    for line_number, source, table, key, value in statements:
        if not table and key and key[0] == "target":
            errors.append(f"{path}:{line_number}: platform cfg: {source}")
        if manifest_dependency_is_native(table, key, value, native_aliases):
            errors.append(f"{path}:{line_number}: native backend detail: {source}")
    return errors


def diagnostic(path: PurePosixPath, text: str, offset: int, label: str, detail: str) -> str:
    line_number = text.count("\n", 0, offset) + 1
    compact = " ".join(detail.split())
    return f"{path}:{line_number}: {label}: {compact}"


def file_errors(
    path: PurePosixPath, text: str, native_aliases: frozenset[str] = frozenset()
) -> list[str]:
    if not is_portable_file(path) or is_backend_file(path):
        return []

    if path.suffix != ".rs":
        scanned = mask_toml_comments(text)
        source_lines = text.splitlines()
        scanned_lines = scanned.splitlines()
        errors = manifest_policy_errors(
            path, text, native_aliases, source_lines, scanned_lines
        )
        for line_number, (line, scan_line) in enumerate(
            zip(source_lines, scanned_lines), 1
        ):
            if PLATFORM_DATA_PATH.search(scan_line):
                errors.append(
                    f"{path}:{line_number}: platform data path: {line.strip()}"
                )
        return errors

    errors: list[str] = []
    scanned, code = scan_rust(text)
    env_imports = rust_env_imports(code)
    if path not in FACADE_ROOTS:
        for offset, expression, expression_code in cfg_expressions(text, code):
            if PLATFORM_CFG.search(expression_code):
                errors.append(
                    diagnostic(path, text, offset, "platform cfg", expression)
                )
        if path.name == "build.rs":
            for offset, access in build_target_accesses(scanned, code, env_imports):
                errors.append(
                    diagnostic(path, text, offset, "platform cfg", access)
                )
        modules, _, _, const_modules, const_imports = env_imports
        const_offsets = set(const_imports)
        const_offsets.update(
            access.start() for access in TARGET_CONST_MODULE.finditer(code)
        )
        for module in modules:
            const_offsets.update(
                access.start()
                for access in re.finditer(
                    rf"\b{re.escape(module)}::consts\b", code
                )
            )
        for module in const_modules:
            const_offsets.update(
                access.start()
                for access in re.finditer(rf"\b{re.escape(module)}::", code)
            )
        const_lines: set[int] = set()
        for offset in sorted(const_offsets):
            line_number = text.count("\n", 0, offset) + 1
            if line_number in const_lines:
                continue
            const_lines.add(line_number)
            errors.append(
                diagnostic(path, text, offset, "platform cfg", "std::env::consts")
            )
    for line_number, (line, scan_line, code_line) in enumerate(
        zip(text.splitlines(), scanned.splitlines(), code.splitlines()), 1
    ):
        stripped = line.strip()
        if any(marker in code_line for marker in NATIVE_MARKERS):
            errors.append(f"{path}:{line_number}: native backend detail: {stripped}")
        if PLATFORM_DATA_PATH.search(scan_line):
            errors.append(f"{path}:{line_number}: platform data path: {stripped}")
    return errors


def source_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for crate in PORTABLE_CRATES:
        directory = root / "crates" / crate
        files.append(directory / "Cargo.toml")
        files.extend(sorted(directory.rglob("*.rs")))
    return files


def validate(root: Path) -> list[str]:
    errors: list[str] = []
    native_aliases = frozenset(
        workspace_native_aliases((root / "Cargo.toml").read_text(encoding="utf-8"))
    )
    for path in source_files(root):
        relative = PurePosixPath(path.relative_to(root).as_posix())
        errors.extend(
            file_errors(relative, path.read_text(encoding="utf-8"), native_aliases)
        )
    return errors


def self_test() -> None:
    core = PurePosixPath("crates/bridge-runtime/src/runtime.rs")
    errors = file_errors(
        core,
        '#[cfg(target_os = "macos")]\n'
        "use objc2::MainThreadMarker;\n"
        'const DATA: &str = "/Users/example/Library/Application Support/app";\n',
    )
    assert len(errors) == 3
    assert "platform cfg" in errors[0]
    assert "native backend detail" in errors[1]
    assert "platform data path" in errors[2]

    multiline = file_errors(
        core,
        '#[cfg(any(\n    target_os = "macos",\n    target_os = "windows",\n))]\nfn native() {}\n',
    )
    assert len(multiline) == 1
    assert "platform cfg" in multiline[0]

    cfg_string_parenthesis = file_errors(
        core, '#[cfg_attr(target_os = "macos", doc = "(")]\nfn native() {}\n'
    )
    assert len(cfg_string_parenthesis) == 1
    assert "platform cfg" in cfg_string_parenthesis[0]

    cfg_raw_string = file_errors(
        core, '#[cfg_attr(target_os = "macos", doc = r"\\")]\nfn native() {}\n'
    )
    assert len(cfg_raw_string) == 1
    assert "platform cfg" in cfg_raw_string[0]
    cfg_after_character = file_errors(
        core,
        "const QUOTE: char = '\"';\n"
        '#[cfg(target_os = "macos")]\nfn native() {}\n',
    )
    assert len(cfg_after_character) == 1
    assert "platform cfg" in cfg_after_character[0]
    assert file_errors(core, '// cfg(target_os = "macos")\nfn portable() {}\n') == []
    assert file_errors(core, 'const DOC: &str = r#"cfg(target_os = "macos")"#;\n') == []
    assert file_errors(core, "// provider uses windows::Win32\n") == []
    assert file_errors(
        core, 'const DOC: &str = "windows::Win32 is unsupported";\n'
    ) == []
    assert file_errors(
        core, '// let home = path.join("Library/Application Support");\n'
    ) == []

    direct_standard_api = file_errors(core, "use std::os::unix::fs::FileExt;\n")
    assert len(direct_standard_api) == 1
    assert "native backend detail" in direct_standard_api[0]

    for path in (
        "/dev/cu.usbmodem1",
        "/dev/uinput",
        "/dev/input/event0",
        "/dev/ttyACM0",
    ):
        device_path = file_errors(core, f'const PORT: &str = "{path}";\n')
        assert len(device_path) == 1
        assert "platform data path" in device_path[0]

    for path in (
        "C:/Users/name/AppData/Local/App",
        "c:/users/name/appdata/local/app",
        "~/Library/Application Support/App",
        "./Library/Application Support/App",
        "/usr/share/app",
        "/usr/local/share/app",
        "file:///Users/name/Library/Application Support/App",
        "file://localhost/Users/name/Library/Application Support/App",
        "file:///usr/share/app",
        "file:///C:/Users/name/AppData",
        "file:///c:/users/name/appdata",
    ):
        data_path = file_errors(core, f'const DATA: &str = "{path}";\n')
        assert len(data_path) == 1
        assert "platform data path" in data_path[0]

    windows_path = file_errors(
        core, 'const DATA: &str = r"C:\\Users\\name\\AppData\\Local\\App";\n'
    )
    assert len(windows_path) == 1
    assert "platform data path" in windows_path[0]
    xdg_data_path = file_errors(core, 'const DATA: &str = ".local/share/app";\n')
    assert len(xdg_data_path) == 1
    assert "platform data path" in xdg_data_path[0]
    xdg_data_variable = file_errors(core, 'const ENV: &str = "XDG_DATA_HOME";\n')
    assert len(xdg_data_variable) == 1
    assert "platform data path" in xdg_data_variable[0]
    assert file_errors(core, 'const HELP: &str = "Press HOME to continue";\n') == []
    assert file_errors(
        core, 'const URL: &str = "https://example.test/tmp/cache";\n'
    ) == []
    assert file_errors(
        core,
        'const URL: &str = "https://example.test/~/Library/Application Support/docs";\n',
    ) == []
    assert file_errors(
        core, 'const URL: &str = "https://example.test/file:///Users/name/docs";\n'
    ) == []
    assert file_errors(
        core, 'const TEXT: &str = "not-file:///Users/name/docs";\n'
    ) == []

    relative_macos_path = file_errors(
        core, 'const DATA: &str = "Library/Application Support/App";\n'
    )
    assert len(relative_macos_path) == 1
    assert "platform data path" in relative_macos_path[0]

    for exact_path in ('"/tmp"', '"/etc"', '"Library/Application Support"'):
        exact_errors = file_errors(core, f"const DATA: &str = {exact_path};\n")
        assert len(exact_errors) == 1
        assert "platform data path" in exact_errors[0]

    joined_path = file_errors(
        core, 'let data = user_home.join("Library/Application Support");\n'
    )
    assert len(joined_path) == 1
    assert "platform data path" in joined_path[0]
    assert file_errors(core, 'let data = user_home.join("Library");\n') == []
    assert file_errors(core, 'let text = words.join(".config");\n') == []
    assert file_errors(core, 'let text = ["a", "b"].join("Library");\n') == []

    manifest_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[target.\'cfg(target_os = "windows")\'.dependencies]\nwindows-sys = "0.60"\n',
    )
    assert len(manifest_errors) == 2
    for target in ("x86_64-pc-windows-msvc", "aarch64-apple-darwin"):
        for table in (
            f"[target.{target}.dependencies]",
            f'["target" . "{target}" . "dependencies"]',
        ):
            target_errors = file_errors(
                PurePosixPath("crates/bridge-core/Cargo.toml"),
                f'{table}\nserde = "1"\n',
            )
            assert len(target_errors) == 1
            assert "platform cfg" in target_errors[0]
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '# cfg(target_os = "windows")\n',
    ) == []
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        'description = "cfg(target_os = windows) is unsupported"\n',
    ) == []

    for dependency in (
        "ashpd",
        "block2",
        "dispatch2",
        "enigo",
        "gtk",
        "hidapi",
        "libei",
        "reis",
        "uinput",
        "windows",
        "windows-targets",
        "zbus",
    ):
        dependency_errors = file_errors(
            PurePosixPath("crates/bridge-core/Cargo.toml"),
            f'[dependencies]\n{dependency} = "1"\n',
        )
        assert len(dependency_errors) == 1
        assert "native backend detail" in dependency_errors[0]
    alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[dependencies]\nnative = { package = "windows", version = "1" }\n',
    )
    assert len(alias_errors) == 1
    assert "native backend detail" in alias_errors[0]
    single_quote_alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies]\nnative = { package = 'windows', version = '1' }\n",
    )
    assert len(single_quote_alias_errors) == 1
    assert "native backend detail" in single_quote_alias_errors[0]
    quoted_key_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[dependencies]\n"windows" = "1"\n',
    )
    assert len(quoted_key_errors) == 1
    assert "native backend detail" in quoted_key_errors[0]
    direct_workspace_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies]\nwindows . workspace = true\n",
    )
    assert len(direct_workspace_errors) == 1
    direct_table_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies . windows]\nworkspace = true\n",
    )
    assert len(direct_table_errors) == 1
    quoted_direct_table_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '["dependencies" . "windows"]\nworkspace = true\n',
    )
    assert len(quoted_direct_table_errors) == 1
    spaced_direct_table_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[ "dependencies" . "windows" ]\nworkspace = true\n',
    )
    assert len(spaced_direct_table_errors) == 1
    dotted_direct_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        'dependencies.windows = "1"\n',
    )
    assert len(dotted_direct_errors) == 1
    inline_direct_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        'dependencies = { windows = "0.60" }\n',
    )
    assert len(inline_direct_errors) == 1
    inline_alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        'dependencies = { native = { package = "windows", version = "0.60" } }\n',
    )
    assert len(inline_alias_errors) == 1
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[package.metadata.docs]\nwindows = "not a dependency"\n'
        'package = "windows"\ntarget.windows = "not target cfg"\n',
    ) == []
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[package.metadata.dependencies]\nwindows = "documentation label"\n'
        '\n[package.metadata.dependencies.windows]\nnote = "not a dependency"\n',
    ) == []
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[package]\ndescription = """\n[dependencies]\n'
        'windows = "not a dependency"\n"""\n',
    ) == []
    assert file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[package]\ndescription = """\nLiteral triple quote: \\"""\n'
        '[dependencies]\nwindows = "not a dependency"\n"""\n',
    ) == []
    escaped_dependency = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[dependencies]\n"wind\\u006fws" = "1"\n',
    )
    assert len(escaped_dependency) == 1
    escaped_package = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[dependencies]\nnative = { package = "wind\\u006fws", version = "1" }\n',
    )
    assert len(escaped_package) == 1
    multiline_package = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        '[dependencies.native]\npackage = """\nwindows"""\nversion = "1"\n',
    )
    assert len(multiline_package) == 1

    portal_import = file_errors(core, "use zbus::Connection;\n")
    assert len(portal_import) == 1
    assert "native backend detail" in portal_import[0]

    aliases = workspace_native_aliases(
        "[workspace.dependencies]\n"
        'native_059 = { package = "windows", version = "1" }\n'
        "portal = {\n  package = 'ashpd',\n  version = '1'\n}\n"
        'native_win . package = "windows"\n'
        'native_win.version = "1"\n'
        "\n[workspace.dependencies.input]\npackage = \"uinput\"\nversion = \"1\"\n"
    )
    assert aliases == {"input", "native_059", "native_win", "portal"}
    spaced_aliases = workspace_native_aliases(
        '[workspace . dependencies]\nnative = { package = "windows", version = "1" }\n'
        '\n[workspace.dependencies . input]\npackage = "uinput"\nversion = "1"\n'
    )
    assert spaced_aliases == {"input", "native"}
    for quote in ('"', "'"):
        quoted_aliases = workspace_native_aliases(
            f"[{quote}workspace{quote} . {quote}dependencies{quote}]\n"
            'native = { package = "windows", version = "1" }\n'
            f"\n[{quote}workspace{quote} . {quote}dependencies{quote} . "
            f"{quote}input{quote}]\npackage = 'uinput'\nversion = '1'\n"
        )
        assert quoted_aliases == {"input", "native"}
    assert workspace_native_aliases(
        'workspace.dependencies.native = { package = "windows", version = "1" }\n'
    ) == {"native"}
    assert workspace_native_aliases(
        'workspace = { dependencies = { native = '
        '{ package = "windows", version = "1" } } }\n'
    ) == {"native"}
    assert workspace_native_aliases(
        '["work\\u0073pace"."dependencies"]\n'
        'native = { package = "windows", version = "1" }\n'
    ) == {"native"}
    assert workspace_native_aliases(
        '[workspace.dependencies.native]\npackage = """\nwindows"""\n'
        'version = "1"\n'
    ) == {"native"}
    assert workspace_native_aliases(
        '[workspace]\ndependencies.native = { package = "windows", version = "1" }\n'
    ) == {"native"}
    assert workspace_native_aliases(
        '[package.metadata]\nworkspace.dependencies.native = '
        '{ package = "windows", version = "1" }\n'
    ) == set()
    assert workspace_native_aliases(
        '[workspace.dependencies]\nserde = "1" # package = "windows"\n'
        '\n[workspace.dependencies.foo]\n# package = "windows"\n'
    ) == set()
    native_aliases = frozenset(aliases)
    alias_manifest_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies]\nnative_059.workspace = true\n",
        native_aliases,
    )
    assert len(alias_manifest_errors) == 1
    assert "native backend detail" in alias_manifest_errors[0]
    dotted_alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies]\nnative_win.workspace = true\n",
        native_aliases,
    )
    assert len(dotted_alias_errors) == 1
    assert "native backend detail" in dotted_alias_errors[0]
    spaced_alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies]\nnative_win . workspace = true\n",
        native_aliases,
    )
    assert len(spaced_alias_errors) == 1
    spaced_alias_table_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies . native_win]\nworkspace = true\n",
        native_aliases,
    )
    assert len(spaced_alias_table_errors) == 1
    full_dotted_alias_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "dependencies.native_win.workspace = true\n",
        native_aliases,
    )
    assert len(full_dotted_alias_errors) == 1
    alias_table_errors = file_errors(
        PurePosixPath("crates/bridge-core/Cargo.toml"),
        "[dependencies.native_059]\nworkspace = true\n",
        native_aliases,
    )
    assert len(alias_table_errors) == 1
    assert "native backend detail" in alias_table_errors[0]
    assert file_errors(core, "use crate::input::InputState;\n", native_aliases) == []

    allowed = '#[cfg(target_os = "macos")]\n'
    assert file_errors(
        PurePosixPath("crates/bridge-output/src/serial_discovery.rs"), allowed
    ) == []
    assert len(
        file_errors(
            PurePosixPath("crates/bridge-output/src/serial_discovery.rs"),
            "use objc2::MainThreadMarker;\n",
        )
    ) == 1
    assert file_errors(
        PurePosixPath("crates/bridge-output/src/serial_discovery/macos.rs"),
        allowed + 'const DATA: &str = "/dev/cu.test";\nuse objc2::MainThreadMarker;\n',
    ) == []
    assert file_errors(PurePosixPath("crates/menu-shell/src/lib.rs"), allowed) == []

    build_script = PurePosixPath("crates/bridge-core/build.rs")
    integration_test = PurePosixPath("crates/bridge-core/tests/native.rs")
    assert is_portable_file(build_script)
    assert is_portable_file(integration_test)
    for target_access in (
        'std::env::var("CARGO_CFG_TARGET_OS")',
        'std::env::var_os("CARGO_CFG_UNIX")',
        'env!("TARGET")',
        'option_env!("TARGET")',
        'std::env::vars().find(|(key, _)| key == "TARGET")',
    ):
        target_errors = file_errors(
            build_script, f"fn main() {{ let _ = {target_access}; }}\n"
        )
        assert len(target_errors) == 1
        assert "platform cfg" in target_errors[0]
    split_enumeration = file_errors(
        build_script,
        "fn main() {\n"
        "    let mut vars = std::env::vars();\n"
        '    if vars.any(|(key, _)| key == "TARGET") {}\n'
        "}\n",
    )
    assert len(split_enumeration) == 1
    assert "platform cfg" in split_enumeration[0]
    reversed_enumeration = file_errors(
        build_script,
        "fn main() {\n"
        "    for (key, _) in std::env::vars() {\n"
        '        if "TARGET" == key {}\n'
        "    }\n"
        "}\n",
    )
    assert len(reversed_enumeration) == 1
    assigned_for_enumeration = file_errors(
        build_script,
        "fn main() {\n"
        "    let vars = std::env::vars();\n"
        "    for (key, _) in vars {\n"
        '        if key == "TARGET" {}\n'
        "    }\n"
        "}\n",
    )
    assert len(assigned_for_enumeration) == 1
    tuple_field_enumeration = file_errors(
        build_script,
        'fn main() { std::env::vars().any(|entry| entry.0 == "TARGET"); }\n',
    )
    assert len(tuple_field_enumeration) == 1
    for viewed_enumeration in (
        'fn main() { std::env::vars().any(|(key, _)| '
        'key.as_str() == "TARGET"); }\n',
        'fn main() { std::env::vars().any(|entry| '
        'entry.0.as_str() == "TARGET"); }\n',
    ):
        viewed_errors = file_errors(build_script, viewed_enumeration)
        assert len(viewed_errors) == 1
    for imported_enumeration in (
        'use std::env::vars; fn main() { '
        'vars().any(|(key, _)| key == "TARGET"); }\n',
        'use std::env::vars as environment; fn main() { '
        'environment().any(|(key, _)| key == "TARGET"); }\n',
        'use std::env as process_env; fn main() { '
        'process_env::vars().any(|(key, _)| key == "TARGET"); }\n',
        'use std::env::*; fn main() { '
        'vars().any(|(key, _)| key == "TARGET"); }\n',
        'use std::env as process_env; use process_env::vars as all_vars; '
        'fn main() { all_vars().any(|(key, _)| key == "TARGET"); }\n',
    ):
        enumeration_errors = file_errors(build_script, imported_enumeration)
        assert len(enumeration_errors) == 1
    assert file_errors(
        build_script,
        'mod env { fn vars() {} } fn main() { '
        'env::vars().any(|(key, _)| key == "TARGET"); }\n',
    ) == []
    unrelated_enumeration = file_errors(
        build_script,
        "fn main() {\n"
        "    let _ = std::env::vars().count();\n"
        '    if command == "TARGET" {}\n'
        "}\n",
    )
    assert unrelated_enumeration == []
    assert file_errors(
        build_script, 'fn main() { let _ = config::var("TARGET"); }\n'
    ) == []
    for imported_access in (
        'use std::env::var; fn main() { let _ = var("TARGET"); }\n',
        'use std::env::{var_os}; fn main() { let _ = var_os("CARGO_CFG_TARGET_OS"); }\n',
        'use std::env::var as read_env; fn main() { let _ = read_env("HOST"); }\n',
        'use std::{env::var as read_env}; fn main() { let _ = read_env("TARGET"); }\n',
        'use std::env::var as read_env; '
        'fn main() { let _ = read_env::<&str>("TARGET"); }\n',
        'use std::{env::{var_os as read_env}}; '
        'fn main() { let _ = read_env("CARGO_CFG_TARGET_OS"); }\n',
        'use std::env as process_env; '
        'fn main() { let _ = process_env::var("TARGET"); }\n',
        'use std::env::{self as process_env}; '
        'fn main() { let _ = process_env::var_os("TARGET"); }\n',
        'use std::env::*; fn main() { let _ = var("TARGET"); }\n',
        'use std as standard; '
        'fn main() { let _ = standard::env::var("TARGET"); }\n',
        'use std::{self as standard}; '
        'fn main() { let _ = standard::env::var_os("HOST"); }\n',
        'use std::*; fn main() { let _ = env::var("TARGET"); }\n',
        'use std as standard; use standard::env::var as read_env; '
        'fn main() { let _ = read_env("TARGET"); }\n',
        'use std::env as process_env; use process_env::var as read_env; '
        'fn main() { let _ = read_env("TARGET"); }\n',
        'fn main() { let _ = std::env::var::<&str>("TARGET"); }\n',
    ):
        imported_errors = file_errors(build_script, imported_access)
        assert len(imported_errors) == 1
        assert "platform cfg" in imported_errors[0]
    target_constant = file_errors(
        build_script,
        'const TARGET_KEY: &str = "TARGET";\n'
        'fn main() { let _ = std::env::var(TARGET_KEY); }\n',
    )
    assert len(target_constant) == 1
    for bound_target in (
        'static TARGET_KEY: &str = "TARGET";\n'
        'fn main() { let _ = std::env::var(TARGET_KEY); }\n',
        'fn main() { let target_key = "TARGET"; '
        'let _ = std::env::var(target_key); }\n',
        'const TARGET_KEY: &str = "TARGET";\n'
        'fn main() { std::env::vars().any(|(key, _)| key == TARGET_KEY); }\n',
    ):
        bound_errors = file_errors(build_script, bound_target)
        assert len(bound_errors) == 1
    assert file_errors(build_script, 'fn main() { println!("TARGET"); }\n') == []
    assert file_errors(
        build_script, 'fn main() { println!("CARGO_CFG_TARGET_OS"); }\n'
    ) == []
    assert file_errors(
        build_script, 'const DOC: &str = r#"std::env::var(\"TARGET\")"#;\n'
    ) == []
    env_const_access = file_errors(core, 'let os = std::env::consts::OS;\n')
    assert len(env_const_access) == 1
    assert "platform cfg" in env_const_access[0]
    env_const_import = file_errors(
        core, 'use std::env::consts::{ARCH, FAMILY};\n'
    )
    assert len(env_const_import) == 1
    assert "platform cfg" in env_const_import[0]
    for const_import in (
        'use std::env as host_env; let _ = host_env::consts::OS;\n',
        'use std::{env::consts::OS};\n',
        'use std as standard; let _ = standard::env::consts::OS;\n',
        'use std::*; let _ = env::consts::OS;\n',
    ):
        const_errors = file_errors(core, const_import)
        assert len(const_errors) == 1
        assert "platform cfg" in const_errors[0]
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        build = root / build_script
        integration = root / integration_test
        build.parent.mkdir(parents=True)
        integration.parent.mkdir(parents=True)
        build.write_text("fn main() {}\n", encoding="utf-8")
        integration.write_text("#[test]\nfn portable() {}\n", encoding="utf-8")
        discovered = source_files(root)
        assert build in discovered
        assert integration in discovered


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    root = Path(__file__).resolve().parent.parent
    errors = validate(root)
    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
