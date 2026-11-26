#!/usr/bin/env python3
"""Send an update package to the watch if the Rust app is running on it.

One Bluetooth connection: handshake, push the package, start the install.
The Rust app answers the handshake with "W301RUST OK"; the stock app answers
"OK". If it is the stock app, nothing is sent and the exit code is 3, so the
caller can fall back to the stock procedure.

usage: send.py PACKAGE        (settings: OPW_MAC, OPW_CHAN, OPW_DIR)
exit:  0 install started, 3 stock firmware is running, 1 failed
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
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    watch = connect()
    try:
        watch.send_cmd("AT^BLUETOOTH", prefix=opw_bt.PFX_AT)
        if b"W301RUST" not in watch.read_response():
            return 3
        # The wearer has to accept the package on the watch. A firmware
        # from before that rule answers a plain ERROR at once.
        watch.send_cmd("AT^W301OFFER", prefix=opw_bt.PFX_AT)
        print("accept the firmware on the watch (tap ACCEPT or press the upper button)", file=sys.stderr)
        answer = watch.read_response(total=40.0)
        if b"DENIED" in answer:
            print("the package was denied on the watch, or not answered in 30 s", file=sys.stderr)
            return 1
        if b"OK" not in answer and b"ERROR" not in answer:
            print("the watch did not answer the offer", file=sys.stderr)
            return 1
        watch.push_file(sys.argv[1], "1:B280STM32_ota.bin")
        watch.send_cmd("FI^OTAUPDATE", prefix=opw_bt.PFX_FI)
        if b"OK" not in watch.read_response():
            print("the watch did not accept the package (see 3:log/rust.log)", file=sys.stderr)
            return 1
        return 0
    finally:
        watch.close()


if __name__ == "__main__":
    sys.exit(main())
