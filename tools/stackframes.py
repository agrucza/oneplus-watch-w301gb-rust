#!/usr/bin/env python3
"""List the functions of the built firmware that reserve the most stack.

    tools/stackframes.py [COUNT] [ELF]

The stack is 64 KB and nothing checks its use when the firmware is built
(see boot::stack and docs/hardware-plan.md). This reads the disassembly
and adds up what each function takes from the stack pointer at its start.
A function that takes tens of kilobytes is the thing to look for: usually
an async function that hands a large object over by value.

It is a lower bound for one function, not the depth of a call chain; the
log line "stack: N of 65536 bytes used at most" is the measurement.
"""
import glob
import re
import subprocess
import sys

count = int(sys.argv[1]) if len(sys.argv) > 1 else 10
elf = sys.argv[2] if len(sys.argv) > 2 else "target/thumbv7em-none-eabihf/release/stm32_app"
sysroot = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True).stdout.strip()
tools = glob.glob(sysroot + "/lib/rustlib/*/bin/llvm-objdump")
if not tools:
    sys.exit("llvm-objdump not found; install it with: rustup component add llvm-tools")
lines = subprocess.run([tools[0], "-d", "--no-show-raw-insn", "-C", elf], capture_output=True, text=True).stdout.split("\n")

frames = {}
name = None
for index, line in enumerate(lines):
    start = re.match(r"^[0-9a-f]+ <(.*)>:$", line)
    if start:
        name, seen = start.group(1), 0
        frames.setdefault(name, 0)
        continue
    if name is None:
        continue
    seen += 1
    if seen > 16:  # only the function's first instructions
        continue
    direct = re.search(r"\tsub(?:\.w|w|s)?\s+sp, (?:sp, )?#(0x[0-9a-f]+|\d+)", line)
    if direct:
        frames[name] += int(direct.group(1), 0)
    # Large frames: the size is loaded into a register first.
    by_register = re.search(r"\tsub(?:\.w)?\s+sp, (?:sp, )?(r\d+|ip)\s*$", line)
    if by_register:
        for earlier in lines[max(0, index - 5):index][::-1]:
            load = re.search(r"\tmov(?:w|\.w|s)?\s+%s, #(0x[0-9a-f]+|\d+)" % by_register.group(1), earlier)
            if load:
                frames[name] += int(load.group(1), 0)
                break

for name, size in sorted(frames.items(), key=lambda item: -item[1])[:count]:
    print("%6d  %s" % (size, re.sub(r"::h[0-9a-f]{16}$", "", name)[:110]))
