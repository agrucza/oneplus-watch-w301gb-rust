#!/usr/bin/env python3
"""Compare the watch's clock with this computer's.

Asks the Rust app for its clock a few times and prints how far the watch is
ahead (+) or behind (-), taken from the question that came back fastest.
Run it twice, some hours apart, to see how fast the clock drifts; run it
before and after a restart to see what a restart costs.

Each run appends one line to target/clock.txt.

usage: clock.py               (settings: OPW_MAC, OPW_CHAN, OPW_DIR)
exit:  0 done, 3 stock firmware is running, 1 failed
"""
import datetime
import os
import re
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
            print("the stock firmware is running; it has no clock query", file=sys.stderr)
            return 3
        best = None
        for _ in range(8):
            asked = time.time()
            watch.send_cmd("AT^W301TIME?", prefix=opw_bt.PFX_AT)
            answer = watch.read_response()
            back = time.time()
            m = re.search(rb"W301TIME:(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)\.(\d{3}) up=(\d+) calr=(0x[0-9a-f]+)", answer)
            if not m:
                print(f"unexpected answer: {answer!r}", file=sys.stderr)
                continue
            stamp = datetime.datetime.strptime(m.group(1).decode(), "%Y-%m-%d %H:%M:%S")
            watch_s = time.mktime(stamp.timetuple()) + int(m.group(2)) / 1000
            trip = back - asked
            # The watch read its clock somewhere between question and answer.
            offset = watch_s - (asked + back) / 2
            if best is None or trip < best[0]:
                best = (trip, offset, int(m.group(3)), m.group(4).decode(), asked)
            time.sleep(0.3)
        if best is None:
            return 1
        trip, offset, up_ms, calr, asked = best
        when = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(asked))
        line = (f"{when}  watch {offset:+.3f} s against this computer "
                f"(within {trip / 2 * 1000:.0f} ms), up {up_ms / 1000:.0f} s, calibration register {calr}")
        print(line)
        os.makedirs("target", exist_ok=True)
        with open("target/clock.txt", "a") as f:
            f.write(line + "\n")
        return 0
    finally:
        watch.close()


if __name__ == "__main__":
    sys.exit(main())
