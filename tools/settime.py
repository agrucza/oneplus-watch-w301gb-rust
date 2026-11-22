#!/usr/bin/env python3
"""Set the watch's clock to this computer's local time.

Works only while the Rust app is running; the stock firmware takes its time
from the phone.

usage: settime.py             (settings: OPW_MAC, OPW_CHAN, OPW_DIR)
exit:  0 clock set, 3 stock firmware is running, 1 failed
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
    watch = connect()
    try:
        watch.send_cmd("AT^BLUETOOTH", prefix=opw_bt.PFX_AT)
        if b"W301RUST" not in watch.read_response():
            print("the stock firmware is running; it takes its time from the phone", file=sys.stderr)
            return 3
        # Send right at the start of a second, so the watch is not up to a
        # second behind.
        time.sleep(1.0 - time.time() % 1.0)
        now = time.strftime("%Y-%m-%d %H:%M:%S")
        watch.send_cmd(f"AT^W301TIME={now}", prefix=opw_bt.PFX_AT)
        if b"OK" not in watch.read_response():
            print("the watch did not accept the time (see 3:log/rust.log)", file=sys.stderr)
            return 1
        print(f"watch clock set to {now}")
        return 0
    finally:
        watch.close()


if __name__ == "__main__":
    sys.exit(main())
