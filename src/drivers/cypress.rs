//! The Cypress Bluetooth controller (reports itself as chip 20721).
//!
//! The chip runs its own Bluetooth stack and application; the STM32 talks to
//! it over USART3 with Cypress's WICED HCI control protocol. Everything here
//! mirrors the stock app (factory build, addresses from its disassembly):
//!
//! * control pins, set up at 0x0805D662:
//!     PB2   output  - goes high first in the power-up sequence
//!     PC6   output  - goes high 50 ms after PB2 (looks like reset release)
//!     PB9   output  - low = "wake", high = "sleep" (BSP_Cypress_wake_by_ST)
//!     PB14  output  - host ready-to-receive, low = ready (driven by hand in
//!                     the stock app; here it is the UART's hardware RTS)
//!     PC7   input   - interrupt line from the chip
//! * power-up sequence at 0x0805B03C
//! * USART3 (0x0808ED2A): 3,000,000 baud, 8 data bits, 2 stop bits, CTS flow
//!   control; TX PC4, RX PC5, CTS PB13; DMA in both directions.
//!
//! Packet, same layout in both directions (stock builder at 0x080A7834):
//! 0x19, opcode (u16 LE: low byte = code, high byte = group), payload length
//! (u16 LE), payload.
//!
//! The chip forgets everything at power-up: it starts on a default address
//! (20:72:1B:20:11:22) without any pairing keys. The host sets both.

use core::fmt::Write;

use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::mode::Async;
use embassy_stm32::usart::{self, RingBufferedUartRx, StopBits, Uart, UartTx};
use embassy_stm32::{bind_interrupts, dma, peripherals, Peri};
use embassy_futures::select::{select, Either};
use embassy_time::{with_timeout, Duration, Instant, Timer};

use crate::services::log::Log;

bind_interrupts!(struct Irqs {
    USART3 => usart::InterruptHandler<peripherals::USART3>;
    DMA1_CHANNEL1 => dma::InterruptHandler<peripherals::DMA1_CH1>;
    DMA1_CHANNEL2 => dma::InterruptHandler<peripherals::DMA1_CH2>;
});

const PACKET_MARKER: u8 = 0x19;
/// Largest payload we accept or send (the stock app sends up to 962).
pub const MAX_PAYLOAD: usize = 1024;

// Commands (opcode = group << 8 | code), as sent by the stock app.
/// Set the chip's Bluetooth address: 6 bytes, last byte of the address first
/// (stock: 0x0806873E).
pub const CMD_SET_LOCAL_BDA: u16 = 0x0003;
/// Hand stored pairing data back to the chip: id (u16 LE) + data
/// (stock: 0x0806881A).
pub const CMD_PUSH_NVRAM_DATA: u16 = 0x0005;
/// Set discoverable / connectable: 2 bytes, 0 or 1 each (stock: 0x0806876E).
pub const CMD_SET_VISIBILITY: u16 = 0x0008;
/// Ask for the chip's Bluetooth address.
pub const CMD_READ_LOCAL_BDA: u16 = 0x000F;

// Events seen so far.
/// Answer to most commands: 1 byte, 0 = accepted.
pub const EVT_COMMAND_STATUS: u16 = 0x0001;
/// The chip has (re)started and is ready for commands.
pub const EVT_DEVICE_STARTED: u16 = 0x0005;
/// Answer to `CMD_READ_LOCAL_BDA`: 6 bytes, first byte of the address first.
pub const EVT_LOCAL_BDA: u16 = 0x000D;

/// Everything the Cypress link uses.
pub struct Resources {
    pub usart: Peri<'static, peripherals::USART3>,
    pub tx: Peri<'static, peripherals::PC4>,
    pub rx: Peri<'static, peripherals::PC5>,
    pub cts: Peri<'static, peripherals::PB13>,
    pub rts: Peri<'static, peripherals::PB14>,
    pub power: Peri<'static, peripherals::PB2>,
    pub reset: Peri<'static, peripherals::PC6>,
    pub wake: Peri<'static, peripherals::PB9>,
    pub irq: Peri<'static, peripherals::PC7>,
    pub rx_dma: Peri<'static, peripherals::DMA1_CH1>,
    pub tx_dma: Peri<'static, peripherals::DMA1_CH2>,
}

/// A received packet. `payload` is valid until the next `Link` call.
pub struct Packet<'a> {
    pub opcode: u16,
    pub payload: &'a [u8],
}

/// The packet link to the powered-up chip.
pub struct Link {
    tx: UartTx<'static, Async>,
    rx: RingBufferedUartRx<'static>,
    buf: &'static mut [u8; 5 + MAX_PAYLOAD + 256],
    len: usize,
    /// Length of the packet handed out by the last `receive`; removed from
    /// the buffer on the next call.
    consumed: usize,
    /// UART receive errors seen so far (framing, noise, overrun).
    pub rx_errors: u32,
    _pins: (Output<'static>, Output<'static>, Output<'static>, Input<'static>),
}

impl Link {
    /// Power the chip up as the stock app does and open the UART.
    pub async fn power_up(r: Resources) -> Option<Self> {
        // Stock delays are 51/51/102 ticks of a 1024 Hz tick.
        let wake = Output::new(r.wake, Level::Low, Speed::Low); // awake
        let mut reset = Output::new(r.reset, Level::Low, Speed::Low);
        let mut power = Output::new(r.power, Level::Low, Speed::Low);
        let irq = Input::new(r.irq, Pull::None);
        Timer::after_millis(50).await;
        power.set_high();
        Timer::after_millis(50).await;
        reset.set_high();
        Timer::after_millis(100).await;

        let mut config = usart::Config::default();
        config.baudrate = 3_000_000;
        config.stop_bits = StopBits::STOP2;
        let uart =
            Uart::new_with_rtscts(r.usart, r.tx, r.rx, r.rts, r.cts, r.tx_dma, r.rx_dma, Irqs, config).ok()?;
        let (tx, rx) = uart.split();
        let ring = cortex_m::singleton!(: [u8; 16384] = [0; 16384])?;
        let buf = cortex_m::singleton!(: [u8; 5 + MAX_PAYLOAD + 256] = [0; 5 + MAX_PAYLOAD + 256])?;
        Some(Self {
            tx,
            rx: rx.into_ring_buffered(ring),
            buf,
            len: 0,
            consumed: 0,
            rx_errors: 0,
            _pins: (wake, reset, power, irq),
        })
    }

    /// Send one command.
    pub async fn send(&mut self, opcode: u16, payload: &[u8]) -> bool {
        let mut header = [PACKET_MARKER, 0, 0, 0, 0];
        header[1..3].copy_from_slice(&opcode.to_le_bytes());
        header[3..5].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        let send = async {
            self.tx.write(&header).await.is_ok() && (payload.is_empty() || self.tx.write(payload).await.is_ok())
        };
        // A chip that holds CTS high would block us forever.
        with_timeout(Duration::from_secs(1), send).await.unwrap_or(false)
    }

    /// Wait up to `limit` for the next packet.
    pub async fn receive(&mut self, limit: Duration) -> Option<Packet<'_>> {
        self.receive_until(limit, false).await
    }

    /// As `receive`, but a change on one of the wake lines (`hal::wake`:
    /// upper button, Apollo, the chip's interrupt line) ends the wait as
    /// well, with `None`. For the main loop's idle wait only: an answer
    /// that is being waited for must not be given up for a button.
    pub async fn receive_idle(&mut self, limit: Duration) -> Option<Packet<'_>> {
        self.receive_until(limit, true).await
    }

    async fn receive_until(&mut self, limit: Duration, edge_ends: bool) -> Option<Packet<'_>> {
        // Drop the packet handed out last time.
        self.buf.copy_within(self.consumed..self.len, 0);
        self.len -= self.consumed;
        self.consumed = 0;

        let end = Instant::now() + limit;
        loop {
            // Resynchronise on the marker, then see if a packet is complete.
            let start = self.buf[..self.len].iter().position(|&b| b == PACKET_MARKER).unwrap_or(self.len);
            self.buf.copy_within(start..self.len, 0);
            self.len -= start;
            if self.len >= 5 {
                let payload = u16::from_le_bytes([self.buf[3], self.buf[4]]) as usize;
                if payload > MAX_PAYLOAD {
                    self.buf[0] = 0; // not a real header; skip this marker
                    continue;
                }
                if self.len >= 5 + payload {
                    self.consumed = 5 + payload;
                    return Some(Packet {
                        opcode: u16::from_le_bytes([self.buf[1], self.buf[2]]),
                        payload: &self.buf[5..5 + payload],
                    });
                }
            }
            let now = Instant::now();
            if now >= end {
                return None;
            }
            let wait = (end - now).min(Duration::from_millis(100));
            let free = self.len..self.buf.len();
            let read = if edge_ends {
                match with_timeout(wait, select(self.rx.read(&mut self.buf[free]), crate::hal::wake::edge())).await {
                    Ok(Either::First(read)) => Ok(read),
                    Ok(Either::Second(())) => return None,
                    Err(timeout) => Err(timeout),
                }
            } else {
                with_timeout(wait, self.rx.read(&mut self.buf[free])).await
            };
            match read {
                Ok(Ok(n)) => self.len += n,
                Ok(Err(_)) => {
                    // The receiver restarts on the next read. Give the rest of
                    // the system a turn first: an error that is still present
                    // makes `read` fail again at once, and a loop of such
                    // failures would never let the watchdog task run.
                    self.rx_errors = self.rx_errors.wrapping_add(1);
                    Timer::after_millis(5).await;
                }
                Err(_) => {} // nothing arrived in time
            }
        }
    }

    /// Send a command and wait for a packet with opcode `answer`, logging
    /// everything else that arrives meanwhile. Returns up to 16 bytes of the
    /// answer's payload, or `None` after one second.
    pub async fn request(&mut self, log: &mut Log, opcode: u16, payload: &[u8], answer: u16) -> Option<([u8; 16], usize)> {
        if !self.send(opcode, payload).await {
            let _ = writeln!(log.stamp(), "cypress: command {:#06x} could not be sent", opcode);
            return None;
        }
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            let Some(packet) = self.receive(Duration::from_millis(200)).await else {
                continue;
            };
            if packet.opcode == answer {
                let mut out = [0u8; 16];
                let n = packet.payload.len().min(16);
                out[..n].copy_from_slice(&packet.payload[..n]);
                return Some((out, n));
            }
            log_packet(log, &packet);
        }
        let _ = writeln!(log.stamp(), "cypress: no answer {:#06x} to command {:#06x}", answer, opcode);
        None
    }

    /// Send a command that is answered with a status byte. True if accepted.
    pub async fn command(&mut self, log: &mut Log, opcode: u16, payload: &[u8]) -> bool {
        match self.request(log, opcode, payload, EVT_COMMAND_STATUS).await {
            Some((status, 1)) if status[0] == 0 => true,
            Some((status, n)) => {
                let _ = writeln!(log.stamp(), "cypress: command {:#06x} refused, status:", opcode);
                log.hex(&status[..n]);
                false
            }
            None => false,
        }
    }
}

/// Write a received packet to the log.
pub fn log_packet(log: &mut Log, packet: &Packet<'_>) {
    let _ = writeln!(
        log.stamp(),
        "cypress event group {:#04x} code {:#04x}, {} bytes",
        packet.opcode >> 8,
        packet.opcode & 0xFF,
        packet.payload.len()
    );
    log.hex(packet.payload);
}
