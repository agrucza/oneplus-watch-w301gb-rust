/* OnePlus Watch W301 - STM32L4R9 (2 MB flash, 640 KB contiguous SRAM)
 *
 *   0x0800_0000 .. 0x0801_DFFF   stm32_boot.bin  (120 KB) - NEVER touched
 *   0x0801_E000 .. 0x081F_FFFF   stm32_app.bin   <- us
 *
 * The bootloader's jump_to_app loads MSP from [0x0801E000] and jumps to
 * [0x0801E004], so our vector table must be the very first thing at
 * 0x0801E000 - which is exactly where cortex-m-rt places it (start of FLASH).
 */
MEMORY
{
  /* Bootloader WriteFlashBin rejects images unless 0x0801E000 + size < 0x08200000.
   * Dual-bank, 4 KB pages; bank 2 starts at 0x08100000 (0xE2000 into the app). */
  FLASH : ORIGIN = 0x0801E000, LENGTH = 0x1E2000 - 4

  /* SRAM1 (192K) + SRAM2 (64K) + SRAM3 (384K) are contiguous.
   * The first 64 KB are reserved for the stack (see below). */
  STACK : ORIGIN = 0x20000000, LENGTH = 64K
  RAM   : ORIGIN = 0x20010000, LENGTH = 576K
}

/* Stack lives *below* .data/.bss and grows down towards 0x2000_0000:
 *  - a stack overflow runs off the bottom of RAM and HardFaults instead of
 *    silently corrupting statics;
 *  - the initial SP (0x2001_0000) stays in the low RAM range, like the stock
 *    app's 0x2000_1000, so it passes the usual bootloader sanity check
 *    `(sp & 0x2FFE0000) == 0x20000000`.
 */
_stack_start = ORIGIN(STACK) + LENGTH(STACK);
_stack_end   = ORIGIN(STACK);
