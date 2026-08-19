#!/usr/bin/env python3
"""Build, assemble, and ad-hoc sign the macOS application bundle."""

from __future__ import annotations

import argparse
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DIST_DIR = ROOT / "dist"
APP_DIR = DIST_DIR / "Steam Controller Bridge.app"
HELPER_NAME = "Steam Controller Bridge Virtual HID Helper.app"
DEFAULT_HELPER_IDENTIFIER = "com.lynxware.steam-controller-bridge.virtual-hid-helper"
SHIPPED_MENU_FEATURES = ("editor", "overlay", "updater")
LOCAL_UPDATE_SENTINEL = b"SC_BRIDGE_LOCAL_UPDATE_DIR"


def run(command: list[str], *, capture_output: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        capture_output=capture_output,
        text=True,
    )


def workspace_version() -> str:
    result = run(
        [
            sys.executable,
            str(ROOT / "tools/check-workspace-versions.py"),
            "--print-version",
        ],
        capture_output=True,
    )
    version = result.stdout.strip()
    if not version:
        raise ValueError("workspace version validator returned no version")
    return version


def build_command() -> list[str]:
    features = ",".join(
        f"sc-bridge-menu/{feature}" for feature in SHIPPED_MENU_FEATURES
    )
    return [
        "cargo",
        "build",
        "--release",
        "-p",
        "sc-bridge-menu",
        "-p",
        "virtual-gamepad",
        "--bins",
        "--no-default-features",
        "--features",
        features,
    ]


def menu_default_features() -> tuple[str, ...]:
    manifest = (ROOT / "apps/sc-bridge-menu/Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^default\s*=\s*\[([^]]*)\]\s*$', manifest, re.MULTILINE)
    if match is None:
        raise ValueError("menu manifest has no one-line default feature list")
    features = tuple(re.findall(r'"([^"]+)"', match.group(1)))
    if not features:
        raise ValueError("menu manifest default feature list is empty")
    return features


def verify_shipped_update_boundary(executable: Path) -> None:
    if LOCAL_UPDATE_SENTINEL in executable.read_bytes():
        raise ValueError(
            "shipped menu executable contains the local update source environment variable"
        )


def remove_bundle(bundle: Path, expected_parent: Path) -> None:
    if bundle.parent.resolve() != expected_parent.resolve() or bundle.name != APP_DIR.name:
        raise ValueError(f"refusing unsafe application bundle path: {bundle}")
    if bundle.is_symlink():
        bundle.unlink()
    elif bundle.exists():
        shutil.rmtree(bundle)


def stamp_plist(path: Path, version: str, identifier: str | None = None) -> None:
    with path.open("rb") as source:
        contents = plistlib.load(source)
    if not isinstance(contents, dict):
        raise ValueError(f"application plist is not a dictionary: {path}")
    contents["CFBundleShortVersionString"] = version
    contents["CFBundleVersion"] = version
    if identifier is not None:
        contents["CFBundleIdentifier"] = identifier
    with path.open("wb") as destination:
        plistlib.dump(contents, destination, sort_keys=False)


def assemble_bundle(version: str) -> None:
    remove_bundle(APP_DIR, DIST_DIR)
    contents = APP_DIR / "Contents"
    executable_dir = contents / "MacOS"
    resources_dir = contents / "Resources"
    executable_dir.mkdir(parents=True)
    resources_dir.mkdir(parents=True)

    shutil.copy2(
        ROOT / "target/release/sc-bridge-menu",
        executable_dir / "sc-bridge-menu",
    )
    plist = contents / "Info.plist"
    shutil.copy2(ROOT / "packaging/macos/Info.plist", plist)
    stamp_plist(plist, version)
    shutil.copy2(
        ROOT / "packaging/macos/MenuBarTemplate.svg",
        resources_dir / "MenuBarTemplate.svg",
    )
    shutil.copy2(
        ROOT / "packaging/macos/AppIcon.icns",
        resources_dir / "AppIcon.icns",
    )
    helper = contents / "Helpers" / HELPER_NAME
    helper_executable_dir = helper / "Contents" / "MacOS"
    helper_executable_dir.mkdir(parents=True)
    shutil.copy2(
        ROOT / "target/release/sc-virtual-hid-helper",
        helper_executable_dir / "sc-virtual-hid-helper",
    )
    helper_plist = helper / "Contents" / "Info.plist"
    shutil.copy2(ROOT / "packaging/macos/VirtualHidHelper.Info.plist", helper_plist)
    stamp_plist(
        helper_plist,
        version,
        os.environ.get("SC_BRIDGE_VIRTUAL_HID_HELPER_IDENTIFIER", DEFAULT_HELPER_IDENTIFIER),
    )
    profile = os.environ.get("SC_BRIDGE_VIRTUAL_HID_PROVISIONING_PROFILE")
    if profile:
        shutil.copy2(profile, helper / "Contents" / "embedded.provisionprofile")


def signing_commands(identity: str, app: Path) -> tuple[list[str], list[str], list[str]]:
    helper = app / "Contents" / "Helpers" / HELPER_NAME
    helper_sign = [
        "/usr/bin/codesign",
        "--force",
        "--sign",
        identity,
        "--entitlements",
        str(ROOT / "packaging/macos/VirtualHidHelper.entitlements"),
        str(helper),
    ]
    outer_sign = ["/usr/bin/codesign", "--force", "--sign", identity, str(app)]
    verify = ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)]
    return helper_sign, outer_sign, verify


def inspect_signed_entitlements(app: Path) -> None:
    helper = app / "Contents" / "Helpers" / HELPER_NAME
    helper_result = run(
        ["/usr/bin/codesign", "-d", "--entitlements", ":-", str(helper)],
        capture_output=True,
    )
    helper_entitlements = helper_result.stdout + helper_result.stderr
    if "com.apple.developer.hid.virtual.device" not in helper_entitlements:
        raise ValueError("signed helper is missing the virtual HID entitlement")
    outer_result = run(
        ["/usr/bin/codesign", "-d", "--entitlements", ":-", str(app / "Contents/MacOS/sc-bridge-menu")],
        capture_output=True,
    )
    outer_entitlements = outer_result.stdout + outer_result.stderr
    if "com.apple.developer.hid.virtual.device" in outer_entitlements:
        raise ValueError("outer menu executable must not receive the virtual HID entitlement")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        dist = root / "dist"
        bundle = dist / APP_DIR.name
        bundle.mkdir(parents=True)
        (bundle / "old").write_text("old", encoding="utf-8")
        remove_bundle(bundle, dist)
        assert not bundle.exists()

        outside = root / APP_DIR.name
        outside.mkdir()
        try:
            remove_bundle(outside, dist)
        except ValueError:
            pass
        else:
            raise AssertionError("unsafe bundle path was accepted")
        assert outside.exists()

        plist = root / "Info.plist"
        with plist.open("wb") as destination:
            plistlib.dump(
                {
                    "CFBundleName": "Fixture",
                    "CFBundleVersion": "old",
                    "CFBundleShortVersionString": "old",
                },
                destination,
            )
        stamp_plist(plist, "1.6.0")
        with plist.open("rb") as source:
            stamped = plistlib.load(source)
        assert stamped["CFBundleName"] == "Fixture"
        assert stamped["CFBundleVersion"] == "1.6.0"
        assert stamped["CFBundleShortVersionString"] == "1.6.0"

        helper_plist = root / "Helper.plist"
        shutil.copy2(ROOT / "packaging/macos/VirtualHidHelper.Info.plist", helper_plist)
        stamp_plist(helper_plist, "1.6.0", "test.helper")
        with helper_plist.open("rb") as source:
            helper_stamped = plistlib.load(source)
        assert helper_stamped["CFBundleIdentifier"] == "test.helper"
        assert helper_stamped["LSMinimumSystemVersion"] == "13.0"

        helper_sign, outer_sign, verify = signing_commands("-", bundle)
        assert "--entitlements" in helper_sign
        assert "--deep" not in helper_sign
        assert "--deep" not in outer_sign
        assert "--deep" in verify and "--strict" in verify
        assert helper_sign[-1].endswith(HELPER_NAME)

        command = build_command()
        assert "--no-default-features" in command
        features = command[command.index("--features") + 1].split(",")
        assert all(feature.startswith("sc-bridge-menu/") for feature in features)
        assert all("local-update-source" not in feature for feature in features)
        selected = tuple(feature.split("/", 1)[1] for feature in features)
        assert selected == SHIPPED_MENU_FEATURES
        assert menu_default_features() == SHIPPED_MENU_FEATURES

        shipped = root / "shipped-menu"
        shipped.write_bytes(b"production update source")
        verify_shipped_update_boundary(shipped)
        local = root / "local-source-menu"
        local.write_bytes(b"prefix " + LOCAL_UPDATE_SENTINEL + b" suffix")
        try:
            verify_shipped_update_boundary(local)
        except ValueError:
            pass
        else:
            raise AssertionError("local update source sentinel was accepted")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    version = workspace_version()
    run(build_command())
    verify_shipped_update_boundary(ROOT / "target/release/sc-bridge-menu")
    assemble_bundle(version)
    identity = os.environ.get("SC_BRIDGE_CODESIGN_IDENTITY", "-")
    helper_sign, outer_sign, verify = signing_commands(identity, APP_DIR)
    run(helper_sign)
    run(outer_sign)
    run(verify)
    inspect_signed_entitlements(APP_DIR)
    print(APP_DIR)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        message = f"macOS application build failed: {error}"
        # Captured stderr is otherwise lost with the CalledProcessError.
        captured = getattr(error, "stderr", None)
        if captured:
            message = f"{message}\n{captured.strip()}"
        print(message, file=sys.stderr)
        raise SystemExit(1)
