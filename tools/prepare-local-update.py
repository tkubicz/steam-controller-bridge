#!/usr/bin/env python3
"""Prepare a signed local firmware catalog for App Center testing."""

from __future__ import annotations

import argparse
import base64
import os
import secrets
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
REPOSITORY_TEMP = ROOT / "temp"
DEFAULT_OUTPUT = REPOSITORY_TEMP / "steam-controller-bridge-local-update"
KEY_ID = "local-development"


def run(
    command: list[str],
    *,
    environment: dict[str, str] | None = None,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        env=environment,
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


def output_directory(value: str) -> Path:
    if not value:
        raise ValueError("refusing empty local update directory")
    output = Path(value).expanduser().resolve(strict=False)
    if output in (Path("/"), ROOT):
        raise ValueError(f"refusing unsafe local update directory: {output}")
    if output.is_relative_to(ROOT) and not output.is_relative_to(REPOSITORY_TEMP):
        raise ValueError(
            f"refusing in-repository update directory outside temp: {output}"
        )
    return output


def require_plain_destination(path: Path) -> None:
    if path.is_symlink():
        raise ValueError(f"refusing symlinked local update output: {path}")
    if path.exists() and not path.is_file():
        raise ValueError(f"local update output is not a regular file: {path}")


def write_text(path: Path, contents: str) -> None:
    require_plain_destination(path)
    path.write_text(contents, encoding="utf-8")


def private_key(path: Path) -> str:
    require_plain_destination(path)
    if not path.exists():
        encoded = base64.b64encode(secrets.token_bytes(32)).decode("ascii")
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w", encoding="ascii") as destination:
            destination.write(f"{encoded}\n")
    path.chmod(0o600)
    encoded = "".join(path.read_text(encoding="ascii").splitlines())
    try:
        decoded = base64.b64decode(encoded, validate=True)
    except ValueError as error:
        raise ValueError(f"invalid local update private key: {path}") from error
    if len(decoded) != 32:
        raise ValueError(f"local update private key is not 32 bytes: {path}")
    return encoded


def launch_command(public_keys: str, output: Path) -> str:
    return " ".join(
        [
            f"SC_BRIDGE_UPDATE_PUBLIC_KEYS={shlex.quote(public_keys)}",
            f"SC_BRIDGE_LOCAL_UPDATE_DIR={shlex.quote(str(output))}",
            "cargo run -p sc-bridge-menu",
        ]
    )


def prepare(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    paths = {
        "private": output / "private-key.b64",
        "public": output / "trusted-public-key.txt",
        "application": output / "steam-controller-bridge-macos.zip",
        "firmware": output / "steam-controller-bridge-xiao-nrf52840.uf2",
        "notes": output / "release-notes.md",
        "manifest": output / "steam-controller-bridge-update-manifest.json",
        "signatures": output / "steam-controller-bridge-update-signatures.json",
    }
    for path in paths.values():
        require_plain_destination(path)

    version = workspace_version()
    run(["make", "-C", "firmware/xiao-nrf52840", "artifacts"])
    shutil.copy2(
        ROOT
        / "firmware/xiao-nrf52840/build/artifacts/steam-controller-bridge-xiao-nrf52840.uf2",
        paths["firmware"],
    )
    write_text(
        paths["application"],
        "Local firmware test catalog. Do not install this placeholder application.\n",
    )
    write_text(
        paths["notes"],
        "Local signed firmware build for hardware acceptance.\n",
    )

    environment = os.environ.copy()
    environment["SC_BRIDGE_UPDATE_PRIVATE_KEY_B64"] = private_key(paths["private"])
    run(
        [
            "cargo",
            "run",
            "--locked",
            "-p",
            "release-updater",
            "--bin",
            "release-manifest",
            "--",
            "--release-tag",
            f"v{version}",
            "--version",
            version,
            "--release-notes",
            str(paths["notes"]),
            "--application",
            str(paths["application"]),
            "--firmware",
            str(paths["firmware"]),
            "--firmware-header",
            "firmware/xiao-nrf52840/src/firmware_version.h",
            "--output",
            str(output),
            "--key-id",
            KEY_ID,
            "--public-key-output",
            str(paths["public"]),
        ],
        environment=environment,
    )

    public_keys = "".join(paths["public"].read_text(encoding="ascii").splitlines())
    print(f"\nLocal signed update source prepared at:\n  {output}\n")
    print("Quit any running menu app, then launch the debug build with:\n")
    print(launch_command(public_keys, output))


def self_test() -> None:
    assert output_directory(str(DEFAULT_OUTPUT)) == DEFAULT_OUTPUT
    assert output_directory(str(REPOSITORY_TEMP / "nested")) == (
        REPOSITORY_TEMP / "nested"
    )
    for unsafe in ("", "/", str(ROOT), str(ROOT / "docs/local-update")):
        try:
            output_directory(unsafe)
        except ValueError:
            pass
        else:
            raise AssertionError(f"unsafe output path was accepted: {unsafe!r}")

    with tempfile.TemporaryDirectory() as temporary:
        external = output_directory(str(Path(temporary) / "catalog"))
        assert not external.is_relative_to(ROOT)
        external.mkdir()
        key_path = external / "private-key.b64"
        first = private_key(key_path)
        assert len(base64.b64decode(first, validate=True)) == 32
        assert stat.S_IMODE(key_path.stat().st_mode) == 0o600
        assert private_key(key_path) == first

        quoted = launch_command("fixture=value", external / "path with spaces")
        assert "'" in quoted
        assert "SC_BRIDGE_LOCAL_UPDATE_DIR=" in quoted


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", nargs="?", default=str(DEFAULT_OUTPUT))
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        return 0

    os.umask(0o077)
    prepare(output_directory(arguments.output))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        message = f"local update preparation failed: {error}"
        # Captured stderr is otherwise lost with the CalledProcessError.
        captured = getattr(error, "stderr", None)
        if captured:
            message = f"{message}\n{captured.strip()}"
        print(message, file=sys.stderr)
        raise SystemExit(1)
