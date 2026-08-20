#!/usr/bin/env python3
"""Keep Linux controller access limited to the supported HID identities."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RULE_DIRECTORY = ROOT / "packaging/linux"
RULE_FILENAME = "60-steam-controller-bridge.rules"
README_PATH = ROOT / "packaging/linux/README.md"
IDENTITY_PATH = ROOT / "crates/steam-controller-device/src/lib.rs"
RUST_U16_CONSTANT = re.compile(
    r"^pub const (?P<name>[A-Z0-9_]+): u16 = 0x(?P<value>[0-9a-fA-F]+);$",
    re.MULTILINE,
)
REQUIRED_IDENTITIES = (
    "PROTEUS_VENDOR_ID",
    "PROTEUS_PRODUCT_ID",
    "STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID",
)


def controller_identities(source: str) -> tuple[tuple[str, int, int], ...]:
    constants = {
        match.group("name"): int(match.group("value"), 16)
        for match in RUST_U16_CONSTANT.finditer(source)
    }
    missing = [name for name in REQUIRED_IDENTITIES if name not in constants]
    if missing:
        raise ValueError(f"missing controller identity constants: {', '.join(missing)}")
    vendor = constants["PROTEUS_VENDOR_ID"]
    return (
        ("0003", vendor, constants["PROTEUS_PRODUCT_ID"]),
        ("0005", vendor, constants["STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID"]),
    )


def expected_rules(source: str, assignment: str) -> tuple[str, ...]:
    return tuple(
        f'SUBSYSTEM=="hidraw", KERNELS=="{bus}:{vendor:04X}:{product:04X}.*", {assignment}'
        for bus, vendor, product in controller_identities(source)
    )


def active_rules(text: str) -> list[str]:
    return [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


def exact_rule_errors(text: str, expected: tuple[str, ...], label: str) -> list[str]:
    actual = active_rules(text)
    errors: list[str] = []

    for rule in expected:
        count = actual.count(rule)
        if count == 0:
            errors.append(f"missing {label} rule: {rule}")
        elif count > 1:
            errors.append(f"duplicate {label} rule: {rule}")

    for rule in actual:
        if rule not in expected:
            errors.append(f"unexpected {label} rule: {rule}")

    return errors


def policy_errors(rule_text: str, identity_source: str) -> list[str]:
    try:
        expected = expected_rules(identity_source, 'TAG+="uaccess"')
    except ValueError as error:
        return [str(error)]
    return exact_rule_errors(rule_text, expected, "controller access")


def rule_set_errors(rule_files: dict[str, str], identity_source: str) -> list[str]:
    errors: list[str] = []
    actual_names = set(rule_files)

    if RULE_FILENAME not in actual_names:
        errors.append(f"missing Linux device access rule file: {RULE_FILENAME}")
    for name in sorted(actual_names - {RULE_FILENAME}):
        errors.append(f"unexpected Linux device access rule file: {name}")

    if RULE_FILENAME in rule_files:
        errors.extend(policy_errors(rule_files[RULE_FILENAME], identity_source))
    return errors


def documentation_errors(readme: str, identity_source: str) -> list[str]:
    try:
        expected = expected_rules(
            identity_source,
            'GROUP="steam-controller-bridge", MODE="0660"',
        )
    except ValueError as error:
        return [str(error)]
    documented = "\n".join(
        line for line in readme.splitlines() if line.startswith('SUBSYSTEM=="hidraw"')
    )
    return exact_rule_errors(documented, expected, "headless fallback")


def self_test() -> None:
    identities = "\n".join(
        (
            "pub const PROTEUS_VENDOR_ID: u16 = 0x28de;",
            "pub const PROTEUS_PRODUCT_ID: u16 = 0x1304;",
            "pub const STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID: u16 = 0x1303;",
        )
    )
    expected = expected_rules(identities, 'TAG+="uaccess"')
    exact = "\n".join(("# identities", *expected, ""))
    assert policy_errors(exact, identities) == []

    duplicate = policy_errors("\n".join((*expected, expected[0])), identities)
    assert duplicate == [f"duplicate controller access rule: {expected[0]}"]

    missing = policy_errors(expected[0], identities)
    assert missing == [f"missing controller access rule: {expected[1]}"]

    broad = policy_errors(
        "\n".join(
            (
                expected[0],
                expected[1],
                'SUBSYSTEM=="hidraw", KERNELS=="0003:28DE:*", TAG+="uaccess"',
            )
        ),
        identities,
    )
    assert broad == [
        'unexpected controller access rule: SUBSYSTEM=="hidraw", '
        'KERNELS=="0003:28DE:*", TAG+="uaccess"'
    ]

    world_writable = policy_errors(
        "\n".join(
            (
                expected[0],
                expected[1],
                'SUBSYSTEM=="hidraw", MODE="0666"',
            )
        ),
        identities,
    )
    assert world_writable == [
        'unexpected controller access rule: SUBSYSTEM=="hidraw", MODE="0666"'
    ]

    drifted = identities.replace("0x1304", "0x1305")
    assert policy_errors(exact, drifted)

    assert rule_set_errors({RULE_FILENAME: exact}, identities) == []
    assert rule_set_errors({}, identities) == [
        f"missing Linux device access rule file: {RULE_FILENAME}"
    ]
    assert rule_set_errors(
        {RULE_FILENAME: exact, "99-open.rules": 'SUBSYSTEM=="hidraw", MODE="0666"'},
        identities,
    ) == ["unexpected Linux device access rule file: 99-open.rules"]

    headless = "\n".join(
        expected_rules(
            identities,
            'GROUP="steam-controller-bridge", MODE="0660"',
        )
    )
    assert documentation_errors(headless, identities) == []
    assert documentation_errors(headless.replace("1304", "1305"), identities)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    identity_source = IDENTITY_PATH.read_text(encoding="utf-8")
    rule_files = {
        path.name: path.read_text(encoding="utf-8")
        for path in RULE_DIRECTORY.glob("*.rules")
    }
    errors = rule_set_errors(rule_files, identity_source)
    errors.extend(
        documentation_errors(README_PATH.read_text(encoding="utf-8"), identity_source)
    )
    if errors:
        for error in errors:
            print(error)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
