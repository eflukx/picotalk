#!/usr/bin/env python3
"""Sets the Finder's bundle bit in MacBinary files, in place.

The Finder shows an application's own icon (its BNDL, FREF and ICN#
resources) only when the file's bundle bit is set. Retro68's Rez does not
set it, so the build sets it here: bit 5 of the Finder flags' high byte,
MacBinary header byte 73, keeping the header CRC valid.

    python3 tools/setbundle.py FILE.bin...
"""
import sys


def crc16(data):
    """CRC-16/XMODEM, as MacBinary II uses for its header."""
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = (crc << 1) ^ 0x1021 if crc & 0x8000 else crc << 1
            crc &= 0xFFFF
    return crc


for path in sys.argv[1:]:
    with open(path, "r+b") as f:
        header = bytearray(f.read(128))
        if len(header) < 128 or header[0] != 0:
            sys.exit(f"{path}: not a MacBinary file")
        # A valid header CRC (MacBinary II, and Retro68's files whatever
        # their version byte says) must stay valid: readers check it.
        had_crc = crc16(header[:124]) == int.from_bytes(header[124:126], "big")
        header[73] |= 0x20
        if had_crc:
            header[124:126] = crc16(header[:124]).to_bytes(2, "big")
        f.seek(0)
        f.write(header)
