#!/usr/bin/env bash
# Poll the SWD port (without resetting the target) until the core answers,
# then identify the chip and show the Rust app's RTT log for a few seconds.
#   tools/swd_poll.sh [seconds to keep trying, default 540]
cd "$(dirname "$0")/.."
CHIP=STM32L4R9ZIYxP
ELF=target/thumbv7em-none-eabihf/release/stm32_app
END=$(( $(date +%s) + ${1:-540} ))
n=0; last=""
while [ "$(date +%s)" -lt "$END" ]; do
    n=$((n+1))
    out=$(timeout 10 probe-rs read --chip $CHIP --protocol swd --speed 1000 b32 0xE0042000 1 2>&1)
    if echo "$out" | grep -qE '^[0-9a-fA-F]{8}$'; then
        echo "[$(date +%T)] try $n: core answers. DBGMCU_IDCODE = $out   (STM32L4R9 = ....0470)"
        echo "  CPUID           : $(timeout 10 probe-rs read --chip $CHIP --protocol swd b32 0xE000ED00 1 2>&1 | tail -1)"
        echo "  vectors @0801E000: $(timeout 10 probe-rs read --chip $CHIP --protocol swd b32 0x0801E000 2 2>&1 | tail -1)"
        echo "  DBGMCU_CR       : $(timeout 10 probe-rs read --chip $CHIP --protocol swd b32 0xE0042004 1 2>&1 | tail -1)"
        echo "--- RTT log (15 s) ---"
        timeout 15 probe-rs attach --chip $CHIP --protocol swd "$ELF" 2>&1 | tail -25
        exit 0
    fi
    msg=$(echo "$out" | grep -vE '^\s*$' | tail -1)
    if [ "$msg" != "$last" ]; then echo "[$(date +%T)] try $n: $msg"; last=$msg; fi
    sleep 1
done
echo "[$(date +%T)] gave up after $n tries; last: $last"
exit 1
