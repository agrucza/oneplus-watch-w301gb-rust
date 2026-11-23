#!/usr/bin/env bash
# What can be checked without the watch:
#   1. the tests of the firmware's pure logic, on the PC (tools/host);
#   2. the stack each function of the built firmware reserves.
# Run before an image goes to the watch.
set -euo pipefail
cd "$(dirname "$0")/.."

(cd tools/host && cargo test --quiet)
cargo build --release --quiet
echo "largest stack frames in the build (the stack is 65536 bytes):"
tools/stackframes.py 6
