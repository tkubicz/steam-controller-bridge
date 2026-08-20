#!/usr/bin/env python3
"""Keep Linux device access limited to supported runtime identities."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RULE_DIRECTORY = ROOT / "packaging/linux"
RULE_FILENAME = "60-steam-controller-bridge.rules"
README_PATH = ROOT / "packaging/linux/README.md"
IDENTITY_PATH = ROOT / "crates/steam-controller-device/src/lib.rs"
FIRMWARE_TARGETS_PATH = ROOT / "crates/release-updater/firmware-targets.json"
RUST_U16_CONSTANT = re.compile(
    r"^pub const (?P<name>[A-Z0-9_]+): u16 = 0x(?P<value>[0-9a-fA-F]+);$",
    re.MULTILINE,
)
REQUIRED_IDENTITIES = (
    "PROTEUS_VENDOR_ID",
    "PROTEUS_PRODUCT_ID",
    "STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID",
)
BRIDGE_TARGET_ID = "seeed-xiao-nrf52840"


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


def controller_rules(source: str, assignment: str) -> tuple[str, ...]:
    return tuple(
        f'SUBSYSTEM=="hidraw", KERNELS=="{bus}:{vendor:04X}:{product:04X}.*", {assignment}'
        for bus, vendor, product in controller_identities(source)
    )


def bridge_identity(source: str) -> tuple[int, int, str, str]:
    data = json.loads(source)
    target = next(
        (target for target in data.get("targets", []) if target.get("id") == BRIDGE_TARGET_ID),
        None,
    )
    if target is None:
        raise ValueError(f"missing firmware target: {BRIDGE_TARGET_ID}")

    try:
        usb = target["application_usb"]
        return (
            int(usb["vendor_id"], 0),
            int(usb["product_id"], 0),
            target["application_manufacturer"],
            target["application_product"],
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"invalid application identity for {BRIDGE_TARGET_ID}") from error


def bridge_rule(source: str, assignment: str) -> str:
    vendor, product, manufacturer, product_name = bridge_identity(source)
    return (
        f'SUBSYSTEM=="tty", ATTRS{{idVendor}}=="{vendor:04x}", '
        f'ATTRS{{idProduct}}=="{product:04x}", '
        f'ATTRS{{manufacturer}}=="{manufacturer}", '
        f'ATTRS{{product}}=="{product_name}", {assignment}, '
        'ENV{ID_MM_DEVICE_IGNORE}="1"'
    )


def expected_rules(
    identity_source: str,
    firmware_targets: str,
    assignment: str,
) -> tuple[str, ...]:
    return (
        *controller_rules(identity_source, assignment),
        bridge_rule(firmware_targets, assignment),
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


def policy_errors(
    rule_text: str, identity_source: str, firmware_targets: str
) -> list[str]:
    try:
        expected = expected_rules(
            identity_source,
            firmware_targets,
            'TAG+="uaccess"',
        )
    except (ValueError, json.JSONDecodeError) as error:
        return [str(error)]
    return exact_rule_errors(rule_text, expected, "device access")


def rule_set_errors(
    rule_files: dict[str, str], identity_source: str, firmware_targets: str
) -> list[str]:
    errors: list[str] = []
    actual_names = set(rule_files)

    if RULE_FILENAME not in actual_names:
        errors.append(f"missing Linux device access rule file: {RULE_FILENAME}")
    for name in sorted(actual_names - {RULE_FILENAME}):
        errors.append(f"unexpected Linux device access rule file: {name}")

    if RULE_FILENAME in rule_files:
        errors.extend(
            policy_errors(
                rule_files[RULE_FILENAME], identity_source, firmware_targets
            )
        )
    return errors


def documentation_errors(
    readme: str, identity_source: str, firmware_targets: str
) -> list[str]:
    try:
        expected = expected_rules(
            identity_source,
            firmware_targets,
            'GROUP="steam-controller-bridge", MODE="0660"',
        )
    except (ValueError, json.JSONDecodeError) as error:
        return [str(error)]
    documented = "\n".join(
        stripped
        for line in readme.splitlines()
        if (stripped := line.strip()).startswith('SUBSYSTEM=="')
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
    firmware_targets = json.dumps(
        {
            "targets": [
                {
                    "id": BRIDGE_TARGET_ID,
                    "application_usb": {
                        "vendor_id": "0x045e",
                        "product_id": "0x028e",
                    },
                    "application_manufacturer": "Lynxware",
                    "application_product": "Steam Controller Bridge",
                }
            ]
        }
    )
    expected = expected_rules(
        identities,
        firmware_targets,
        'TAG+="uaccess"',
    )
    exact = "\n".join(("# identities", *expected, ""))
    assert policy_errors(exact, identities, firmware_targets) == []

    duplicate = policy_errors(
        "\n".join((*expected, expected[0])), identities, firmware_targets
    )
    assert duplicate == [f"duplicate device access rule: {expected[0]}"]

    missing = policy_errors("\n".join(expected[:2]), identities, firmware_targets)
    assert missing == [f"missing device access rule: {expected[2]}"]

    broad = policy_errors(
        "\n".join(
            (
                *expected,
                'SUBSYSTEM=="hidraw", KERNELS=="0003:28DE:*", TAG+="uaccess"',
            )
        ),
        identities,
        firmware_targets,
    )
    assert broad == [
        'unexpected device access rule: SUBSYSTEM=="hidraw", '
        'KERNELS=="0003:28DE:*", TAG+="uaccess"'
    ]

    world_writable = policy_errors(
        "\n".join(
            (
                *expected,
                'SUBSYSTEM=="hidraw", MODE="0666"',
            )
        ),
        identities,
        firmware_targets,
    )
    assert world_writable == [
        'unexpected device access rule: SUBSYSTEM=="hidraw", MODE="0666"'
    ]

    drifted = identities.replace("0x1304", "0x1305")
    assert policy_errors(exact, drifted, firmware_targets)

    broad_bridge = expected[2].replace(
        ', ATTRS{manufacturer}=="Lynxware", ATTRS{product}=="Steam Controller Bridge"',
        "",
    )
    assert policy_errors(
        "\n".join((*expected[:2], broad_bridge)), identities, firmware_targets
    )
    assert policy_errors(
        exact.replace(', ENV{ID_MM_DEVICE_IGNORE}="1"', ""),
        identities,
        firmware_targets,
    )

    drifted_bridge = firmware_targets.replace("0x028e", "0x028f")
    assert policy_errors(exact, identities, drifted_bridge)

    assert rule_set_errors({RULE_FILENAME: exact}, identities, firmware_targets) == []
    assert rule_set_errors({}, identities, firmware_targets) == [
        f"missing Linux device access rule file: {RULE_FILENAME}"
    ]
    assert rule_set_errors(
        {RULE_FILENAME: exact, "99-open.rules": 'SUBSYSTEM=="hidraw", MODE="0666"'},
        identities,
        firmware_targets,
    ) == ["unexpected Linux device access rule file: 99-open.rules"]

    headless = "\n".join(
        expected_rules(
            identities,
            firmware_targets,
            'GROUP="steam-controller-bridge", MODE="0660"',
        )
    )
    assert documentation_errors(headless, identities, firmware_targets) == []
    assert documentation_errors(
        headless.replace("1304", "1305"), identities, firmware_targets
    )
    assert documentation_errors(
        f'{headless}\n  SUBSYSTEM=="tty", MODE="0666"',
        identities,
        firmware_targets,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    identity_source = IDENTITY_PATH.read_text(encoding="utf-8")
    firmware_targets = FIRMWARE_TARGETS_PATH.read_text(encoding="utf-8")
    rule_files = {
        path.name: path.read_text(encoding="utf-8")
        for path in RULE_DIRECTORY.glob("*.rules")
    }
    errors = rule_set_errors(rule_files, identity_source, firmware_targets)
    errors.extend(
        documentation_errors(
            README_PATH.read_text(encoding="utf-8"),
            identity_source,
            firmware_targets,
        )
    )
    if errors:
        for error in errors:
            print(error)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
