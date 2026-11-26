#!/usr/bin/env python3
"""Put a small file (440 bytes at most) into 3:log/ on the watch, under a
short name of capitals and digits (8.3). Works with the Rust app only.

For the files the firmware reads but does not carry: the speaker
amplifier's patch and settings (see src/drivers/audio.rs).

usage: put.py FILE NAME       (settings: OPW_MAC, OPW_CHAN, OPW_DIR)
exit:  0 written, 3 stock firmware is running, 1 failed
"""
import os
import sys
import time

sys.path.insert(0, os.environ["OPW_DIR"])
import opw_bt  # noqa: E402


def connect():
    last = None
    for _ in range(5):
        watch = opw_bt.Watch(os.environ["OPW_MAC"], chan=int(os.environ.get("OPW_CHAN", "2")), handshake=False)
        try:
            watch.connect()
            return watch
        except OSError as e:  # the watch needs a moment after a previous connection
            last = e
            watch.close()
            time.sleep(3)
    sys.exit(f"could not connect: {last}")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], "rb") as f:
        content = f.read()
    name = sys.argv[2]
    if not 0 < len(content) <= 440:
        sys.exit(f"{sys.argv[1]} has {len(content)} bytes; 1 to 440 can be put")
    watch = connect()
    try:
        watch.send_cmd("AT^BLUETOOTH", prefix=opw_bt.PFX_AT)
        if b"W301RUST" not in watch.read_response():
            return 3
        watch.send_cmd(f"AT^W301PUT={name},{content.hex()}", prefix=opw_bt.PFX_AT)
        if b"OK" not in watch.read_response():
            print("the watch did not take the file (see 3:log/rust.log)", file=sys.stderr)
            return 1
        print(f"3:log/{name} written, {len(content)} bytes")
        return 0
    finally:
        watch.close()


if __name__ == "__main__":
    sys.exit(main())
