#!/usr/bin/env python3
"""Validate the installation receipt page in compiled firmware artifacts."""

from __future__ import annotations

import argparse
import re
import struct
import subprocess
import sys
from pathlib import Path


PAGE_SIZE = 4096
PAGE_MAGIC = b"SCIRCP01"
HEADER_SIZE = 16
SLOT_SIZE = 64
SLOT_COUNT = 2
UF2_MAGIC_START_0 = 0x0A324655
UF2_MAGIC_START_1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY_ID = 0x00002000


def fail(message: str) -> None:
    raise ValueError(message)


def command_output(command: list[str]) -> str:
    result = subprocess.run(command, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        detail = result.stderr.strip() or f"exit status {result.returncode}"
        fail(f"{command[0]} failed: {detail}")
    return result.stdout


def receipt_symbol(elf: Path, nm: str) -> tuple[int, int]:
    matches: list[tuple[int, int]] = []
    for line in command_output([nm, "-S", "-a", str(elf)]).splitlines():
        fields = line.split()
        if len(fields) >= 4 and fields[-1].endswith("kInstallReceiptPageE"):
            matches.append((int(fields[0], 16), int(fields[1], 16)))
    if len(matches) != 1:
        fail(f"expected one receipt page symbol, found {len(matches)}")
    address, size = matches[0]
    if address % PAGE_SIZE != 0:
        fail(f"receipt page address {address:#x} is not 4 KiB aligned")
    if size != PAGE_SIZE:
        fail(f"receipt page size is {size}, expected {PAGE_SIZE}")
    return address, size


def validate_sections(elf: Path, objdump: str, address: int) -> None:
    output = command_output([objdump, "-h", str(elf)]).splitlines()
    sections: list[tuple[str, int, int, int, str]] = []
    header = re.compile(
        r"^\s*\d+\s+(\S+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+"
        r"([0-9a-fA-F]+)\s+[0-9a-fA-F]+\s+2\*\*(\d+)"
    )
    for index, line in enumerate(output):
        match = header.match(line)
        if match is None:
            continue
        name, size, _vma, lma, alignment = match.groups()
        flags = output[index + 1].strip() if index + 1 < len(output) else ""
        sections.append((name, int(size, 16), int(lma, 16), int(alignment), flags))

    receipt = [section for section in sections if section[0] == ".install_receipt"]
    if len(receipt) != 1:
        fail(f"expected one .install_receipt section, found {len(receipt)}")
    name, size, lma, alignment, flags = receipt[0]
    if (size, lma, alignment) != (PAGE_SIZE, address, 12):
        fail(
            f"{name} has size/address/alignment {size:#x}/{lma:#x}/{alignment}, "
            f"expected {PAGE_SIZE:#x}/{address:#x}/12"
        )
    if "ALLOC" not in flags or "LOAD" not in flags:
        fail("receipt section is not present in the load image")

    receipt_end = address + PAGE_SIZE
    for other_name, other_size, other_lma, _, other_flags in sections:
        if other_name == name or "ALLOC" not in other_flags or "LOAD" not in other_flags:
            continue
        other_end = other_lma + other_size
        if other_lma < receipt_end and address < other_end:
            fail(f"load section {other_name} shares the receipt flash page")


def parse_hex(path: Path) -> dict[int, int]:
    memory: dict[int, int] = {}
    base = 0
    for line_number, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.startswith(":"):
            fail(f"invalid Intel HEX record at line {line_number}")
        record = bytes.fromhex(line[1:])
        if len(record) < 5 or sum(record) & 0xFF:
            fail(f"invalid Intel HEX checksum at line {line_number}")
        length = record[0]
        offset = int.from_bytes(record[1:3], "big")
        kind = record[3]
        data = record[4 : 4 + length]
        if len(data) != length:
            fail(f"truncated Intel HEX record at line {line_number}")
        if kind == 0:
            for index, byte in enumerate(data):
                memory[base + offset + index] = byte
        elif kind == 2:
            base = int.from_bytes(data, "big") << 4
        elif kind == 4:
            base = int.from_bytes(data, "big") << 16
        elif kind == 1:
            break
    return memory


def parse_uf2(path: Path, expected_family: int) -> dict[int, int]:
    data = path.read_bytes()
    if not data or len(data) % 512 != 0:
        fail("UF2 is not a non-empty sequence of 512-byte blocks")
    memory: dict[int, int] = {}
    for block_index in range(len(data) // 512):
        block = data[block_index * 512 : (block_index + 1) * 512]
        start0, start1, flags = struct.unpack_from("<III", block, 0)
        target, payload_size = struct.unpack_from("<II", block, 12)
        family = struct.unpack_from("<I", block, 28)[0]
        end = struct.unpack_from("<I", block, 508)[0]
        if (start0, start1, end) != (
            UF2_MAGIC_START_0,
            UF2_MAGIC_START_1,
            UF2_MAGIC_END,
        ):
            fail(f"UF2 block {block_index} has invalid magic")
        if not flags & UF2_FLAG_FAMILY_ID or family != expected_family:
            fail(
                f"UF2 block {block_index} family is {family:#010x}, "
                f"expected {expected_family:#010x}"
            )
        if payload_size > 476:
            fail(f"UF2 block {block_index} has an oversized payload")
        payload = block[32 : 32 + payload_size]
        for index, byte in enumerate(payload):
            address = target + index
            previous = memory.get(address)
            if previous is not None and previous != byte:
                fail(f"UF2 contains conflicting data at {address:#x}")
            memory[address] = byte
    return memory


def count_page_magic(memory: dict[int, int]) -> int:
    """Count markers in reassembled memory, including across UF2 blocks."""
    return sum(
        1
        for address, byte in memory.items()
        if byte == PAGE_MAGIC[0]
        and all(
            memory.get(address + offset) == expected
            for offset, expected in enumerate(PAGE_MAGIC)
        )
    )


def page_bytes(memory: dict[int, int], address: int, artifact: str) -> bytes:
    missing = [offset for offset in range(PAGE_SIZE) if address + offset not in memory]
    if missing:
        fail(f"{artifact} omits {len(missing)} bytes from the receipt page")
    return bytes(memory[address + offset] for offset in range(PAGE_SIZE))


def validate_blank_page(page: bytes, revision: int, artifact: str) -> None:
    if page[:8] != PAGE_MAGIC:
        fail(f"{artifact} receipt magic does not match")
    if int.from_bytes(page[8:10], "little") != 1:
        fail(f"{artifact} receipt format is not 1")
    if int.from_bytes(page[10:12], "little") != revision:
        fail(f"{artifact} compiled firmware revision does not match {revision}")
    slots_end = HEADER_SIZE + SLOT_SIZE * SLOT_COUNT
    if page[HEADER_SIZE:slots_end] != b"\xff" * (SLOT_SIZE * SLOT_COUNT):
        fail(f"{artifact} contains a non-blank installation receipt slot")
    if page[12:HEADER_SIZE] != b"\xff" * 4 or page[slots_end:] != b"\xff" * (
        PAGE_SIZE - slots_end
    ):
        fail(f"{artifact} contains non-blank reserved receipt space")


def firmware_revision_source(source: str, origin: str) -> int:
    # Same rule as release-manifest.rs: trim each line and accept exactly one
    # canonical declaration ending in exactly one semicolon.
    marker = "constexpr uint16_t kFirmwareRevision = "
    declarations = [
        line.strip()
        for line in source.splitlines()
        if line.strip().startswith(marker)
    ]
    if len(declarations) != 1:
        fail(
            f"expected one kFirmwareRevision declaration in {origin}, "
            f"found {len(declarations)}"
        )
    match = re.fullmatch(
        r"constexpr uint16_t kFirmwareRevision = ([0-9]+);", declarations[0]
    )
    if match is None:
        fail(f"malformed kFirmwareRevision declaration in {origin}")
    revision = int(match.group(1))
    if not 0 < revision <= 0xFFFF:
        fail(f"firmware revision {revision} is outside the uint16_t range")
    return revision


def firmware_revision(header: Path) -> int:
    return firmware_revision_source(header.read_text(), str(header))


def self_test() -> None:
    assert (
        firmware_revision_source(
            "constexpr uint16_t kFirmwareRevision = 2;\n", "fixture"
        )
        == 2
    )
    for invalid in (
        "constexpr uint16_t kFirmwareRevision = 2;;\n",
        "constexpr uint16_t kFirmwareRevision = 2; trailing\n",
        "constexpr uint16_t kFirmwareRevision = 2;\n"
        "constexpr uint16_t kFirmwareRevision = 3;\n",
        "constexpr uint16_t kFirmwareRevision = 2;\n"
        "constexpr uint16_t kFirmwareRevision = invalid;\n",
    ):
        try:
            firmware_revision_source(invalid, "fixture")
        except ValueError:
            pass
        else:
            raise AssertionError(f"invalid firmware revision was accepted: {invalid!r}")

    crossing = {
        0x100 + offset: byte for offset, byte in enumerate(PAGE_MAGIC)
    }
    assert count_page_magic(crossing) == 1
    crossing[0x200] = PAGE_MAGIC[0]
    assert count_page_magic(crossing) == 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--elf", type=Path)
    parser.add_argument("--hex", dest="hex_path", type=Path)
    parser.add_argument("--uf2", type=Path)
    parser.add_argument("--nm")
    parser.add_argument("--objdump")
    parser.add_argument("--family-id", type=lambda value: int(value, 0))
    parser.add_argument("--version-header", type=Path)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0

    required = {
        "--elf": args.elf,
        "--hex": args.hex_path,
        "--uf2": args.uf2,
        "--nm": args.nm,
        "--objdump": args.objdump,
        "--family-id": args.family_id,
        "--version-header": args.version_header,
    }
    missing = [option for option, value in required.items() if value is None]
    if missing:
        parser.error(f"the following arguments are required: {', '.join(missing)}")

    revision = firmware_revision(args.version_header)
    address, _ = receipt_symbol(args.elf, args.nm)
    validate_sections(args.elf, args.objdump, address)
    hex_page = page_bytes(parse_hex(args.hex_path), address, "Intel HEX")
    uf2_memory = parse_uf2(args.uf2, args.family_id)
    magic_count = count_page_magic(uf2_memory)
    uf2_page = page_bytes(uf2_memory, address, "UF2")
    validate_blank_page(hex_page, revision, "Intel HEX")
    validate_blank_page(uf2_page, revision, "UF2")
    if hex_page != uf2_page:
        fail("Intel HEX and UF2 receipt pages differ")
    if magic_count != 1:
        fail(f"UF2 contains {magic_count} receipt markers, expected exactly one")
    print(f"firmware receipt page verified at {address:#x}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(f"firmware artifact validation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
