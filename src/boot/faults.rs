//! How the app ends when something goes wrong, and why the last boot ended.
//!
//! Without a debugger every failure must end in a reset, never in a hang:
//! the reset is what takes us back to the stock firmware. Before resetting,
//! each handler rewrites the return ticket with its own report byte, so the
//! bootloader log (`err=` in 3:log/ota.log) shows how the app ended:
//!   142 panic, 143 hard fault, 141 unexpected interrupt.
//! A watchdog reset or a power loss leaves the last regular report.

use core::fmt;
use core::panic::PanicInfo;

use cortex_m::peripheral::SCB;
use cortex_m_rt::{exception, ExceptionFrame};

use crate::boot::ticket;

const RCC_CSR: *mut u32 = 0x4002_1094 as *mut u32;

fn leave(report_byte: u8) -> ! {
    let _ = ticket::request_install(ticket::RETURN_TO, report_byte);
    SCB::sys_reset()
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    leave(0x8E)
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    leave(0x8F)
}

#[exception]
unsafe fn DefaultHandler(_irqn: i16) {
    leave(0x8D)
}

/// Write why the processor was last reset, then clear the record so the
/// next boot shows only its own cause. Every reset sets "pin"; the other
/// flags say what pulled it. "pin" alone means the reset line was driven
/// from outside the processor.
pub fn write_reset_cause(out: &mut impl fmt::Write) {
    let csr = unsafe { RCC_CSR.read_volatile() };
    unsafe { RCC_CSR.write_volatile(csr | 1 << 23) }; // RMVF: clear the flags
    let _ = out.write_str("reset cause:");
    let names = [
        (31, " low-power"),
        (30, " window-watchdog"),
        (29, " watchdog"),
        (28, " software"),
        (27, " brown-out"),
        (26, " pin"),
        (25, " option-bytes"),
        (24, " firewall"),
    ];
    for (bit, name) in names {
        if csr & 1 << bit != 0 {
            let _ = out.write_str(name);
        }
    }
    let _ = writeln!(out, " (RCC_CSR={:#010x})", csr);
}
