#!/usr/bin/env python3
"""Build, assemble, and ad-hoc sign the macOS application bundle."""

from __future__ import annotations

import argparse
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DIST_DIR = ROOT / "dist"
APP_DIR = DIST_DIR / "Steam Controller Bridge.app"


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


def remove_bundle(bundle: Path, expected_parent: Path) -> None:
    if bundle.parent.resolve() != expected_parent.resolve() or bundle.name != APP_DIR.name:
        raise ValueError(f"refusing unsafe application bundle path: {bundle}")
    if bundle.is_symlink():
        bundle.unlink()
    elif bundle.exists():
        shutil.rmtree(bundle)


def stamp_plist(path: Path, version: str) -> None:
    with path.open("rb") as source:
        contents = plistlib.load(source)
    if not isinstance(contents, dict):
        raise ValueError(f"application plist is not a dictionary: {path}")
    contents["CFBundleShortVersionString"] = version
    contents["CFBundleVersion"] = version
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    version = workspace_version()
    run(["cargo", "build", "--release", "-p", "sc-bridge-menu"])
    assemble_bundle(version)
    run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(APP_DIR)])
    run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(APP_DIR)])
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
