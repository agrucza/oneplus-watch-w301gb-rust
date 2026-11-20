#!/usr/bin/env bash
# Build the Rust app, wrap it into an OTA package and install it through the
# stock firmware's Bluetooth OTA.
#
#   tools/ota.sh            build + package only
#   tools/ota.sh send       build + package + push + trigger install
#   tools/ota.sh trigger    only trigger the install of an already pushed package
#   tools/ota.sh stock      tell the running Rust app to return to the stock firmware
#
# `send` finds out which firmware is running. To the Rust app it sends a
# compact package over a single connection (a few seconds). To the stock app
# it sends the padded package with the stock procedure (about two minutes).
#
# Settings come from the environment or from the untracked file `local.env`
# in the project root (see `local.env.example`):
#   OPW_MAC       the watch's Bluetooth address (required; also built into the app)
#   OPW_CHAN      RFCOMM channel                  (default 2)
#   OPW_DIR       folder with opw_bt.py           (default ~/opwatch/work)
#   OPW_TEMPLATE  known-good uncompressed package (default $OPW_DIR/B280STM32_ota.swdfull.bin)
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f local.env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./local.env
    set +a
fi
: "${OPW_MAC:?set OPW_MAC (copy local.env.example to local.env)}"
OPW_DIR=${OPW_DIR:-$HOME/opwatch/work}
OPW_TEMPLATE=${OPW_TEMPLATE:-$OPW_DIR/B280STM32_ota.swdfull.bin}
OPW_CHAN=${OPW_CHAN:-2}
OUT=target/B280STM32_ota.rust.bin
OUT_COMPACT=target/B280STM32_ota.rust.compact.bin

cargo build --release
cargo objcopy --release -- -O binary target/stm32_app.bin
python3 tools/mkota.py "$OPW_TEMPLATE" target/stm32_app.bin "$OUT"
python3 tools/mkota.py --compact "$OPW_TEMPLATE" target/stm32_app.bin "$OUT_COMPACT"
export OPW_MAC OPW_CHAN OPW_DIR

# One opw_bt.py call = one RFCOMM connection. The watch needs a moment to
# release the channel after the previous connection closes, so pause first
# and retry a refused connect a few times.
bt() {
    local try
    for try in 1 2 3 4 5; do
        sleep 3
        if python3 "$OPW_DIR/opw_bt.py" "$@" --mac "$OPW_MAC" --chan "$OPW_CHAN"; then
            return 0
        fi
        echo "[ota.sh] '$1 ${2:-}' failed (try $try/5), retrying..." >&2
    done
    return 1
}

case "${1:-}" in
send)
    rc=0
    python3 tools/send.py "$OUT_COMPACT" || rc=$?
    if [ "$rc" = 3 ]; then
        echo "[ota.sh] stock firmware is running: using the stock procedure" >&2
        bt push "$OUT" 1:B280STM32_ota.bin
        bt cmd "FI^SETUNCOMPFLAG"
        bt cmd "FI^OTAUPDATE"
    elif [ "$rc" != 0 ]; then
        exit "$rc"
    fi
    ;;
stock) # Rust app is running: ask it to hand back to the stock firmware
    bt cmd "AT^W301STOCK"
    ;;
trigger) # package is already on the watch: only start the install
    bt cmd "FI^SETUNCOMPFLAG"
    bt cmd "FI^OTAUPDATE"
    ;;
esac
