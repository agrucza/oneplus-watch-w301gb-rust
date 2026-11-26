#!/usr/bin/env python3
"""Ask the watch's wearer for a yes to a file transfer over Bluetooth.

The Rust app takes no file and gives none unless the wearer accepted it
on the watch shortly before (a tap on ACCEPT, or the upper button). This
asks, and waits for the answer. The stock app and a Rust app from before
that rule need no yes; for them this does nothing.

usage: offer.py GET|PUT       (settings: OPW_MAC, OPW_CHAN, OPW_DIR)
exit:  0 accepted or not needed, 1 denied or failed
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
    if len(sys.argv) != 2 or sys.argv[1] not in ("GET", "PUT"):
        sys.exit(__doc__)
    watch = connect()
    try:
        watch.send_cmd("AT^BLUETOOTH", prefix=opw_bt.PFX_AT)
        if b"W301RUST" not in watch.read_response():
            return 0
        watch.send_cmd(f"AT^W301OFFER={sys.argv[1]}", prefix=opw_bt.PFX_AT)
        print("accept the transfer on the watch (tap ACCEPT or press the upper button)", file=sys.stderr)
        answer = watch.read_response(total=40.0)
        if b"DENIED" in answer:
            print("the transfer was denied on the watch, or not answered in 30 s", file=sys.stderr)
            return 1
        if b"OK" not in answer and b"ERROR" not in answer:
            print("the watch did not answer the offer", file=sys.stderr)
            return 1
        return 0
    finally:
        watch.close()


if __name__ == "__main__":
    sys.exit(main())
