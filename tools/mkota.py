#!/usr/bin/env python3
"""Wrap stm32_app.bin into an uncompressed B280STM32_ota.bin for the stock OTA.

Package layout (from the working self-built packages):
    0x00..0xB3   "~ZZZ" archive header + one entry "1:ota/stm32/stm32_app.bin"
    0xB4..       payload (the app image), length in the entry at 0x76 (LE u32)
    last 4       CRC32 (zlib) of the payload, little-endian

Only payload and CRC are regenerated; the header is taken unchanged from a
known-good package (TEMPLATE). The header contains further fields whose meaning
is unknown, so the image is padded to the template's payload size instead of
changing the size field.

usage: mkota.py TEMPLATE_PACKAGE APP_BIN OUT_PACKAGE
"""
import struct
import sys
import zlib

HDR = 0xB4
SIZE_OFF = 0x76
KEEP_TAIL = 8  # stock app ends in an 8-byte trailer; kept in case something checks it


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    tpl = open(sys.argv[1], "rb").read()
    app = open(sys.argv[2], "rb").read()

    if tpl[:4] != b"~ZZZ":
        sys.exit("template: not an uncompressed ~ZZZ package")
    size = struct.unpack_from("<I", tpl, SIZE_OFF)[0]
    if len(tpl) != HDR + size + 4:
        sys.exit("template: size field does not match file length")
    payload = tpl[HDR:HDR + size]
    if zlib.crc32(payload) != struct.unpack_from("<I", tpl, HDR + size)[0]:
        sys.exit("template: payload CRC mismatch - not a known-good package")
    if b"1:ota/stm32/stm32_app.bin" not in tpl[:HDR]:
        sys.exit("template: entry is not 1:ota/stm32/stm32_app.bin")

    if len(app) > size - KEEP_TAIL:
        sys.exit(f"app is {len(app)} bytes, max {size - KEEP_TAIL}")
    sp, reset = struct.unpack_from("<II", app, 0)
    if not (0x20000000 < sp <= 0x200A0000) or not (reset & 1) or not (0x0801E000 < reset < 0x08200000):
        sys.exit(f"app: implausible vector table (sp={sp:#x}, reset={reset:#x})")

    new = app + b"\xff" * (size - KEEP_TAIL - len(app)) + payload[-KEEP_TAIL:]
    out = tpl[:HDR] + new + struct.pack("<I", zlib.crc32(new))
    open(sys.argv[3], "wb").write(out)
    print(f"{sys.argv[3]}: {len(out)} bytes, app {len(app)} bytes, sp={sp:#x} reset={reset:#x}, "
          f"crc32={zlib.crc32(new):08x}")


if __name__ == "__main__":
    main()
