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
/// Make the chip forget a record: its id (u16 LE). Only ever sent for a
/// device the wearer removes, together with clearing its slot in the
/// pairing file (stock: 0x08068922).
pub const CMD_DELETE_NVRAM_DATA: u16 = 0x0006;
/// Allow pairing: 1 byte, 1 = allowed. The stock app sends it once after
/// the chip has started and never takes it back (0x0802AF58).
pub const CMD_SET_PAIRING_MODE: u16 = 0x0009;
/// Answer the chip's question whether a pairing's number is right: the
/// peer's address, last byte first, then 1 for yes or 0 for no (stock:
/// 0x080689F0).
pub const CMD_USER_CONFIRMATION: u16 = 0x000B;
/// The name the watch is found under: its bytes, no end mark (stock:
/// 0x0806886C).
pub const CMD_SET_LOCAL_NAME: u16 = 0x0012;

// Events seen so far.
/// Answer to most commands: 1 byte, 0 = accepted.
pub const EVT_COMMAND_STATUS: u16 = 0x0001;
/// The chip has (re)started and is ready for commands.
pub const EVT_DEVICE_STARTED: u16 = 0x0005;
/// Answer to `CMD_READ_LOCAL_BDA`: 6 bytes, first byte of the address first.
pub const EVT_LOCAL_BDA: u16 = 0x000D;
/// A record the host is to keep and hand back after every start of the
/// chip: id (u16 LE), then the peer's address and its keys. Never to be
/// written to the log.
pub const EVT_NVRAM_DATA: u16 = 0x0004;
/// A pairing has ended; the stock app shows a failure if the first byte
/// is 5 and does nothing otherwise (0x0802BBFC).
pub const EVT_PAIRING_COMPLETE: u16 = 0x0008;
/// The chip asks whether a pairing's number is right: the peer's address,
/// first byte first (measured: the same order as in the record that
/// follows), then the number (u32 LE).
pub const EVT_USER_CONFIRMATION: u16 = 0x000B;

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
    /// The UART's two halves; `None` while the link is closed (`close`).
    uart: Option<(UartTx<'static, Async>, RingBufferedUartRx<'static>)>,
    /// What the UART is made from, to open it again.
    parts: Parts,
    /// The receiver's ring of bytes; lent to the receiver while it exists.
    ring: *mut [u8; RING],
    /// While the link is closed: our transmit line held at its idle
    /// level, so that the chip reads no noise as bytes.
    parked: Option<Output<'static>>,
    /// Our "ready to receive" line to the chip (PB14, the chip's CTS):
    /// low = ready. Driven by hand, as in the stock app (0x0805D8B8),
    /// and not by the UART: it says "ready" only once reception runs,
    /// so that a chip that is waiting to send cannot start before we
    /// listen.
    ready: Output<'static>,
    buf: &'static mut [u8; 5 + MAX_PAYLOAD + 256],
    len: usize,
    /// Length of the packet handed out by the last `receive`; removed from
    /// the buffer on the next call.
    consumed: usize,
    /// UART receive errors seen so far (framing, noise, overrun).
    pub rx_errors: u32,
    _pins: (Output<'static>, Output<'static>, Output<'static>, Input<'static>),
}

/// The pieces of the chip the UART is built on.
struct Parts {
    usart: Peri<'static, peripherals::USART3>,
    tx: Peri<'static, peripherals::PC4>,
    rx: Peri<'static, peripherals::PC5>,
    rx_dma: Peri<'static, peripherals::DMA1_CH1>,
    tx_dma: Peri<'static, peripherals::DMA1_CH2>,
}

/// How many bytes the receiver can hold between two reads.
const RING: usize = 16384;

impl Link {
    /// Build the UART from its parts.
    fn open_uart(parts: &Parts, ring: *mut [u8; RING]) -> Option<(UartTx<'static, Async>, RingBufferedUartRx<'static>)> {
        let mut config = usart::Config::default();
        config.baudrate = 3_000_000;
        config.stop_bits = StopBits::STOP2;
        // Safety: the parts are used by one UART at a time; the one
        // before, if there was one, has been dropped (`close`), and with
        // it the only other borrow of the ring.
        let uart = unsafe {
            Uart::new(
                parts.usart.clone_unchecked(),
                parts.tx.clone_unchecked(),
                parts.rx.clone_unchecked(),
                parts.tx_dma.clone_unchecked(),
                parts.rx_dma.clone_unchecked(),
                Irqs,
                config,
            )
            .ok()?
        };
        // The chip's "ready" line to us (PB13) still holds our sending
        // back, by the UART itself: the HAL has no constructor with that
        // line alone, so the bit is set here (USART_CR3.CTSE, which may
        // only change while the UART is off; RM0432 ch. 44).
        let regs = embassy_stm32::pac::USART3;
        regs.cr1().modify(|w| w.set_ue(false));
        regs.cr3().modify(|w| w.set_ctse(true));
        regs.cr1().modify(|w| w.set_ue(true));
        let (tx, rx) = uart.split();
        let mut rx = rx.into_ring_buffered(unsafe { &mut *ring });
        // Listening from now on, before the chip is told that we are.
        rx.start();
        Some((tx, rx))
    }

    /// PB13 as the UART's "clear to send" input (alternate function 7).
    /// Done once; the HAL does not know the pin and leaves it alone.
    fn cts_pin_to_uart() {
        const GPIOB: usize = 0x4800_0400;
        unsafe {
            crate::hal::reg::modify((GPIOB + 0x24) as *mut u32, 0xF << 20, 7 << 20); // AFRH, pin 13
            crate::hal::reg::modify(GPIOB as *mut u32, 0b11 << 26, 0b10 << 26); // MODER: alternate function
            crate::hal::reg::modify((GPIOB + 0x0C) as *mut u32, 0b11 << 26, 0); // PUPDR: no pull
        }
    }

    /// Whether the UART is open.
    pub fn is_open(&self) -> bool {
        self.uart.is_some()
    }

    /// Close the UART and tell the chip that its host sleeps (the wake
    /// line high, "sleep" in the stock app's `BSP_Cypress_wake_by_ST`).
    /// Nothing can be sent or received until `open`. The HAL's receiver
    /// forbids the processor's Stop mode for as long as it exists, so
    /// this is what lets the processor stop.
    ///
    /// A stopgap for measuring what Stop mode saves: the chip is not
    /// asked whether it has something to say, and what it sends meanwhile
    /// is lost. The stock app's way of sleeping with the chip has not
    /// been read.
    ///
    /// The lines the UART gives up are not left floating: our transmit
    /// line stays at its idle level, so that the chip reads no noise as
    /// bytes, and our "ready to receive" line says no, so that the chip
    /// keeps what it has to say until the link is open again.
    pub fn close(&mut self) {
        self.close_uart();
        self.set_host_sleeps(true);
    }

    /// The chip's line to us (PC7): high while it wants its host awake
    /// (the stock app takes its rising edge as "wake", 0x0805DD22, and
    /// lets the link sleep only while it reads low, 0x080969E2).
    pub fn chip_wants_host(&self) -> bool {
        self._pins.3.is_high()
    }

    /// Our line to the chip (PB9): high tells it that its host sleeps,
    /// low that it is awake (the stock app: 0x0805D896, called with 1
    /// when the link goes to sleep and 0 when it wakes).
    pub fn set_host_sleeps(&mut self, sleeps: bool) {
        self._pins.0.set_level(if sleeps { Level::High } else { Level::Low });
    }

    /// Switch the chip off and on again as at start, with the UART built
    /// afresh. The chip then needs its set-up again
    /// (`services::bluetooth::set_up`). False if the UART could not be
    /// opened.
    pub async fn power_cycle(&mut self) -> bool {
        self.close_uart();
        self.set_host_sleeps(false);
        self._pins.1.set_low(); // reset
        self._pins.2.set_low(); // power
        Timer::after_millis(50).await;
        self._pins.2.set_high();
        Timer::after_millis(50).await;
        self._pins.1.set_high();
        Timer::after_millis(100).await;
        self.open_uart_again()
    }

    /// Take the UART down, with our transmit line held at its idle level
    /// and "not ready to receive" towards the chip. The wake line is left
    /// as it is.
    pub fn close_uart(&mut self) {
        self.ready.set_high();
        self.uart = None;
        self.len = 0;
        self.consumed = 0;
        // Safety: the UART that used this pin has just been dropped.
        self.parked = Some(unsafe { Output::new(self.parts.tx.clone_unchecked(), Level::High, Speed::Low) });
    }

    /// Build the UART again after `close_uart`. The wake line is left as
    /// it is. False if it could not be set up.
    pub fn open_uart_again(&mut self) -> bool {
        if self.uart.is_none() {
            self.parked = None;
            self.uart = Self::open_uart(&self.parts, self.ring);
        }
        // Reception runs: now the chip may send.
        if self.uart.is_some() {
            self.ready.set_low();
        }
        self.uart.is_some()
    }

    /// Open the UART again after `close`. False if it could not be set up.
    pub fn open(&mut self) -> bool {
        let opened = self.open_uart_again();
        self.set_host_sleeps(false);
        opened
    }

    /// Power the chip up as the stock app does and open the UART.
    pub async fn power_up(r: Resources) -> Option<Self> {
        // Stock delays are 51/51/102 ticks of a 1024 Hz tick.
        let wake = Output::new(r.wake, Level::Low, Speed::Low); // awake
        let mut reset = Output::new(r.reset, Level::Low, Speed::Low);
        let mut power = Output::new(r.power, Level::Low, Speed::Low);
        let irq = Input::new(r.irq, Pull::None);
        let mut ready = Output::new(r.rts, Level::High, Speed::Low); // not ready yet
        let _ = r.cts; // set up below, by hand
        Self::cts_pin_to_uart();
        Timer::after_millis(50).await;
        power.set_high();
        Timer::after_millis(50).await;
        reset.set_high();
        Timer::after_millis(100).await;

        let parts = Parts { usart: r.usart, tx: r.tx, rx: r.rx, rx_dma: r.rx_dma, tx_dma: r.tx_dma };
        let ring: *mut [u8; RING] = cortex_m::singleton!(: [u8; RING] = [0; RING])?;
        let buf = cortex_m::singleton!(: [u8; 5 + MAX_PAYLOAD + 256] = [0; 5 + MAX_PAYLOAD + 256])?;
        let uart = Self::open_uart(&parts, ring)?;
        ready.set_low();
        Some(Self {
            uart: Some(uart),
            parts,
            ring,
            parked: None,
            ready,
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
        let Some((tx, _)) = self.uart.as_mut() else {
            return false;
        };
        let send = async { tx.write(&header).await.is_ok() && (payload.is_empty() || tx.write(payload).await.is_ok()) };
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
            let Some((_, rx)) = self.uart.as_mut() else {
                // Closed: nothing can arrive.
                Timer::after(wait).await;
                continue;
            };
            let read = if edge_ends {
                match with_timeout(wait, select(rx.read(&mut self.buf[free]), crate::hal::wake::edge())).await {
                    Ok(Either::First(read)) => Ok(read),
                    Ok(Either::Second(())) => return None,
                    Err(timeout) => Err(timeout),
                }
            } else {
                with_timeout(wait, rx.read(&mut self.buf[free])).await
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
    // A record to keep holds keys: those never go to the log.
    if packet.opcode == EVT_NVRAM_DATA {
        return;
    }
    log.hex(packet.payload);
}
