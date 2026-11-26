//! Sound out of the speaker: the audio codec (Cirrus CS47L15), the
//! speaker amplifier (Awinic AW8896) and the STM32's audio port (SAI1),
//! set up for one sound file and switched off again after it, as the
//! stock app does for its factory speaker test (`wav_play`, 0x080BCACC in
//! B.70). Its everyday sounds take another way, through the Bluetooth
//! chip; this one needs nothing but what is on the STM32.
//!
//! Two parts work together. `Audio` belongs to the main task, which has
//! the sensor bus the amplifier is on: `start` brings codec and
//! amplifier up for a file and `stop` takes them down. The samples are
//! sent by a task of their own (`player`), so that a sound of some
//! seconds does not hold the rest of the firmware up; it says when it
//! is through (`finished`), and can be cut short (`cut`).
//!
//! The chain, each link read in the stock app and since seen to answer
//! on the watch (2026-11-26):
//!
//! - Codec: control over SPI2 (PB10 clock, PC1 out, PC2 in, PB12 select;
//!   mode 0). A transfer is four address bytes (top bit set for a read),
//!   two of padding, then the value, two bytes below address 0x3000 and
//!   four from there on (0x080BD81E, 0x080BD774). PB1 high wakes it (this
//!   board revision), PB6 is its reset, PA3 carries its 3 MHz clock from
//!   timer 2 (0x08031016). Register 0 reads 0x6370. For playback it gets
//!   one file of settings from the storage chip
//!   (`packetA/codec/adsp_for_playback_48.bin`: a length, then pairs of
//!   address and value; 0x080BDA28): a reset, its clocks from the 3 MHz,
//!   three audio ports, and what the STM32 sends on the second port mixed
//!   to the first, which feeds the amplifier. The file is written without
//!   a pause in the stock app; here the codec's "boot done" bit (0x1880,
//!   bit 7) is waited for after the reset, as its datasheet asks. Volume
//!   is the codec's: 0x703 and 0x70B for the STM32's port, 0x80 = 0 dB,
//!   two units a dB.
//! - Amplifier at 0x36 on the sensor bus: PG6 high, PG10 its reset
//!   (active high). One-byte registers, two-byte values, high byte
//!   first. It takes its clock from the codec's bit clock. Its start
//!   (0x080D91E0 and what that calls, and the maker's own Linux driver,
//!   which does the same): audio format, a table of 18 registers, PLL
//!   locked, the ROM version read from its memory, then a patch for its
//!   processor and a block of settings written into its memory, its
//!   watchdog seen to run, power up, PLL locked again, unmute.
//! - SAI1 block A (PA8 clock, PA9 frame, PA10 data): slave transmitter,
//!   the codec gives the clocks; 16-bit samples in two slots of 16, frame
//!   sync low for the first, one bit early; a one-channel sound goes out
//!   on both (0x080901D4).
//!
//! The patch and the settings are not in this firmware. They are files
//! on the log volume, as the maker's Linux driver loads its
//! `aw8896_fw_d.bin`, `aw8896_fw_e.bin` and `aw8896_cfg.bin`: here
//! `LOG/AW8896FD.BIN` or `LOG/AW8896FE.BIN` (by the ROM version) and
//! `LOG/AW8896CF.BIN`, put there once with `tools/ota.sh put`. Without
//! them nothing is played.
//!
//! Where this departs from the stock app, each time to the careful side:
//! the amplifier stays muted from its table on until everything is
//! loaded (the stock app's table unmutes and three transfers later it
//! mutes again); the amplifier's processor stays in the path (the
//! factory test bypasses it and plays 21 dB louder); the patch is
//! written in pieces of 64 bytes, not 128, because this bus gives a
//! transfer 10 ms.

use core::fmt::Write;

use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::mode::Blocking;
use embassy_stm32::sai::{self, Sai};
use embassy_stm32::spi::{self, Spi};
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_stm32::{bind_interrupts, dma, peripherals, Peri};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};

use crate::boot::watchdog;
use crate::drivers::light::Bus;
use crate::drivers::psram;
use crate::services::log::Log;
use crate::services::storage::{self, Partition};
use crate::services::wav::{self, Wav};

bind_interrupts!(struct Irqs {
    DMA2_CHANNEL1 => dma::InterruptHandler<peripherals::DMA2_CH1>;
    DMA2_CHANNEL2 => dma::InterruptHandler<peripherals::DMA2_CH2>;
});

pub struct Resources {
    pub amplifier_supply: Peri<'static, peripherals::PG6>,
    pub amplifier_reset: Peri<'static, peripherals::PG10>,
    pub codec_reset: Peri<'static, peripherals::PB6>,
    pub codec_select: Peri<'static, peripherals::PB12>,
    pub codec_spi: Peri<'static, peripherals::SPI2>,
    pub codec_clock: Peri<'static, peripherals::PB10>,
    pub codec_out: Peri<'static, peripherals::PC1>,
    pub codec_in: Peri<'static, peripherals::PC2>,
    pub master_clock_timer: Peri<'static, peripherals::TIM2>,
    pub master_clock: Peri<'static, peripherals::PA3>,
    pub port: Port,
}

/// The audio port, which the task that sends the samples has.
pub struct Port {
    pub port: Peri<'static, peripherals::SAI1>,
    pub clock: Peri<'static, peripherals::PA8>,
    pub frame: Peri<'static, peripherals::PA9>,
    pub data: Peri<'static, peripherals::PA10>,
    pub dma: Peri<'static, peripherals::DMA2_CH1>,
    /// The microphone's samples from the codec (block B of the port).
    pub input: Peri<'static, peripherals::PB5>,
    pub input_dma: Peri<'static, peripherals::DMA2_CH2>,
}

/// The codec's and the amplifier's side, without the port.
struct Control {
    amplifier_supply: Peri<'static, peripherals::PG6>,
    amplifier_reset: Peri<'static, peripherals::PG10>,
    codec_reset: Peri<'static, peripherals::PB6>,
    codec_select: Peri<'static, peripherals::PB12>,
    codec_spi: Peri<'static, peripherals::SPI2>,
    codec_clock: Peri<'static, peripherals::PB10>,
    codec_out: Peri<'static, peripherals::PC1>,
    codec_in: Peri<'static, peripherals::PC2>,
    master_clock_timer: Peri<'static, peripherals::TIM2>,
    master_clock: Peri<'static, peripherals::PA3>,
}

/// What is switched on while a sound plays; given up, it all goes off.
struct Live {
    codec: CodecBus<'static>,
    select: Output<'static>,
    _clock: SimplePwm<'static, peripherals::TIM2>,
    _amplifier_supply: Output<'static>,
    _amplifier_reset: Output<'static>,
    _codec_reset: Output<'static>,
    /// The amplifier was started (not so for a recording).
    amplifier: bool,
}

/// What to play.
#[derive(Clone, Copy)]
pub enum Sound {
    /// A WAV file on the image volume.
    File(&'static str),
    /// What was last recorded from the microphone.
    Recording,
}

/// Work for the player task.
#[derive(Clone, Copy)]
enum Job {
    /// A file's samples, at the place the head names.
    File(Partition, &'static str, Wav),
    /// The recording in memory.
    Recording(Wav),
    /// Record from the microphone.
    Record,
}

/// A recording: 48 kHz, one channel, as the stock app's own microphone
/// test records (0x080BE402), and two seconds as there (0x080BE61C). The
/// first fifth of a second is left out: the microphone's supply and the
/// input's high-pass filter settle in it (the stock app keeps it).
const RECORD_RATE: u32 = 48_000;
const RECORD_SAMPLES: usize = 2 * RECORD_RATE as usize;
const RECORD_SKIP: usize = RECORD_RATE as usize / 5;
const _: () = assert!(RECORD_SAMPLES * 2 <= psram::RECORDING_SIZE);
/// How many samples the recording in memory has.
static RECORDED: AtomicU32 = AtomicU32::new(0);
/// Its largest sample, by which it is made louder when played.
static RECORDED_PEAK: AtomicU32 = AtomicU32::new(0);
/// The microphone input's register (IN1L control) and its value for
/// recording: analog, +31 dB. The codec's file for playing has +15 dB
/// (0x809E), at which speech from an arm's length reached 1,100 of
/// 32,768 at most (measured 2026-11-26); +31 dB is what the stock app's
/// second set of voice files has (0x80BE). The file's value is replaced
/// as the file is written, since the gain is not to change while the
/// input is on (datasheet).
const INPUT_REGISTER: u32 = 0x0310;
const INPUT_RECORDING: u32 = 0x80BE;
/// Played back, a recording is made louder until its largest sample is
/// at this (9 dB below full), by this factor at most. At 3 dB below
/// full, speech sounded overdriven from the speaker at the loudest
/// volume, though the recording was far from full (heard 2026-11-26).
const REPLAY_PEAK: u32 = 11_500;
const REPLAY_MOST: u32 = 8;

/// The factor a recording is played back with.
fn replay_gain() -> u32 {
    (REPLAY_PEAK / RECORDED_PEAK.load(Ordering::Relaxed).max(1)).clamp(1, REPLAY_MOST)
}

/// The recording's place in memory.
///
/// Safety: for the player task alone, one use at a time.
unsafe fn recording() -> &'static mut [i16] {
    core::slice::from_raw_parts_mut(psram::RECORDING as *mut i16, RECORD_SAMPLES)
}

/// How a sound went.
#[derive(Clone, Copy)]
pub struct Outcome {
    /// Samples sent, both channels counted.
    pub sent: u32,
    /// How often the port ran dry: gaps in the sound.
    pub dry: u32,
    /// It ended before its time: no clock from the codec, or the file.
    pub failed: bool,
    /// It was cut short on request.
    pub cut: bool,
    /// It was a recording, of `sent` samples; `dry` then counts the times
    /// samples were lost.
    pub recorded: bool,
    /// A recording's largest sample and the mean of all (without sign).
    pub peak: u16,
    pub level: u16,
}

static JOB: Signal<CriticalSectionRawMutex, Job> = Signal::new();
static DONE: Signal<CriticalSectionRawMutex, Outcome> = Signal::new();
static CUT: AtomicBool = AtomicBool::new(false);

/// How the sound that was playing went, once it is through.
pub fn finished() -> Option<Outcome> {
    DONE.try_take()
}

/// End the sound that is playing; `finished` reports it shortly.
pub fn cut() {
    CUT.store(true, Ordering::Relaxed);
}

const AMPLIFIER: u8 = 0x36;
/// The codec's settings for playing at 48 kHz and at 44.1 kHz, on the
/// image volume. They differ in the clock's numbers and nothing else.
const CODEC_FILES: [(u32, &str); 2] = [(48_000, "packetA/codec/adsp_for_playback_48.bin"), (44_100, "packetA/codec/adsp_for_playback_441.bin")];
/// The amplifier's files on the log volume (see the top of this file).
const PATCH_FILES: [&str; 2] = ["LOG/AW8896FD.BIN", "LOG/AW8896FE.BIN"];
const SETTINGS_FILE: &str = "LOG/AW8896CF.BIN";
/// The quietest volume worth having: the codec's mixer value the stock
/// app's own speaker test uses, 22 dB below full (0x080BCB78). Two units
/// are a dB, 0x80 is full.
pub const VOLUME_QUIET: u32 = 0x0054;

/// The amplifier's table of start values (the stock app's, 0x081E3058),
/// register and value, with the last one changed to stay muted.
const AMPLIFIER_TABLE: [(u8, u16); 18] = [
    (0x06, 0x0039),
    (0x07, 0x00A0),
    (0x09, 0x0401),
    (0x0A, 0x6464),
    (0x20, 0x0007),
    (0x21, 0x2101),
    (0x22, 0x0000),
    (0x23, 0x1F1F),
    (0x60, 0x1AA3),
    (0x61, 0xD6FE),
    (0x62, 0x307F),
    (0x63, 0x0002),
    (0x64, 0x0000),
    (0x65, 0x0482),
    (0x66, 0x0144),
    (0x67, 0x0500),
    (0x04, 0x0240),
    (0x08, 0xA00F),
];

type CodecBus<'a> = Spi<'a, Blocking, spi::mode::Master>;

/// Write a codec register.
fn codec_write(spi: &mut CodecBus<'_>, select: &mut Output<'_>, address: u32, value: u32) -> bool {
    let a = (address & 0x7FFF_FFFF).to_be_bytes();
    let v = value.to_be_bytes();
    let mut frame = [a[0], a[1], a[2], a[3], 0, 0, 0, 0, 0, 0];
    let length = if address < 0x3000 {
        frame[6..8].copy_from_slice(&v[2..]);
        8
    } else {
        frame[6..10].copy_from_slice(&v);
        10
    };
    select.set_low();
    let done = spi.blocking_write(&frame[..length]).is_ok();
    select.set_high();
    done
}

/// Read a two-byte codec register.
fn codec_read(spi: &mut CodecBus<'_>, select: &mut Output<'_>, address: u32) -> u16 {
    let a = (address | 0x8000_0000).to_be_bytes();
    let mut frame = [a[0], a[1], a[2], a[3], 0, 0, 0, 0];
    select.set_low();
    let _ = spi.blocking_transfer_in_place(&mut frame);
    select.set_high();
    u16::from_be_bytes([frame[6], frame[7]])
}

fn amplifier_read(bus: &mut Bus, register: u8) -> Option<u16> {
    let mut value = [0u8; 2];
    bus.blocking_write_read(AMPLIFIER, &[register], &mut value).ok().map(|()| u16::from_be_bytes(value))
}

fn amplifier_write(bus: &mut Bus, register: u8, value: u16) -> bool {
    let v = value.to_be_bytes();
    bus.blocking_write(AMPLIFIER, &[register, v[0], v[1]]).is_ok()
}

/// Set and clear bits of an amplifier register.
fn amplifier_change(bus: &mut Bus, register: u8, clear: u16, set: u16) -> bool {
    match amplifier_read(bus, register) {
        Some(value) => amplifier_write(bus, register, value & !clear | set),
        None => false,
    }
}

/// Write a file's words into the amplifier's memory from `at` on: the
/// address once, then the words in pieces, each word high byte first
/// (the files hold them low byte first).
fn amplifier_load(bus: &mut Bus, at: u16, words: &[u8]) -> bool {
    if !amplifier_write(bus, 0x40, at) {
        return false;
    }
    for piece in words.chunks(64) {
        let mut frame = [0u8; 65];
        frame[0] = 0x41;
        for (pair, out) in piece.chunks_exact(2).zip(frame[1..].chunks_exact_mut(2)) {
            out[0] = pair[1];
            out[1] = pair[0];
        }
        if bus.blocking_write(AMPLIFIER, &frame[..1 + piece.len()]).is_err() {
            return false;
        }
    }
    true
}

/// Read a whole small file. `None` if it is not there, empty, or longer
/// than `into`.
fn read_file<'a>(volume: Partition, path: &str, into: &'a mut [u8]) -> Option<&'a [u8]> {
    let mut reader = storage::FileReader::open(volume, path).ok()?;
    let length = reader.len() as usize;
    let read = if length == 0 || length > into.len() { None } else { reader.read(&mut into[..length]).ok() };
    reader.close();
    (read == Some(length)).then(|| &into[..length])
}

pub struct Audio {
    r: Control,
    live: Option<Live>,
}

impl Audio {
    /// The driver for the main task, and the port for the player task.
    pub fn new(r: Resources) -> (Self, Port) {
        let control = Control {
            amplifier_supply: r.amplifier_supply,
            amplifier_reset: r.amplifier_reset,
            codec_reset: r.codec_reset,
            codec_select: r.codec_select,
            codec_spi: r.codec_spi,
            codec_clock: r.codec_clock,
            codec_out: r.codec_out,
            codec_in: r.codec_in,
            master_clock_timer: r.master_clock_timer,
            master_clock: r.master_clock,
        };
        (Self { r: control, live: None }, r.port)
    }

    /// True from `start` to `stop`.
    pub fn playing(&self) -> bool {
        self.live.is_some()
    }

    /// Bring codec and amplifier up for the sound file `path` on the
    /// volume `image` and have the player task send it, at `volume` (the
    /// codec's mixer value). `codec_awake` is PB1, low between sounds;
    /// `files` is the volume with the amplifier's files. Every step is
    /// logged; the first one that fails ends it, with everything off
    /// again. True if the sound is on its way: then `stop` has to follow
    /// once `finished` says so.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(&mut self, codec_awake: &mut Output<'static>, bus: &mut Bus, image: Partition, files: Partition, sound: Sound, volume: u32, log: &mut Log) -> bool {
        if self.live.is_some() {
            return false;
        }
        let (path, wav) = match sound {
            Sound::File(path) => {
                // What the file holds.
                let mut head = [0u8; 1024];
                let wav = match storage::FileReader::open(image, path) {
                    Ok(mut reader) => {
                        let n = reader.read(&mut head).unwrap_or(0);
                        reader.close();
                        wav::parse(&head[..n])
                    }
                    Err(_) => None,
                };
                (path, wav.filter(|w| w.bits == 16 && (1..=2).contains(&w.channels)))
            }
            Sound::Recording => {
                let samples = RECORDED.load(Ordering::Relaxed);
                ("the recording", (samples > 0).then_some(Wav { channels: 1, rate: RECORD_RATE, bits: 16, data_at: 0, data_len: samples * 2 }))
            }
        };
        let _ = writeln!(log.stamp(), "sound: {} at volume {:#06x}", path, volume);
        if matches!(sound, Sound::Recording) {
            let _ = writeln!(log.stamp(), "sound: the recording is played {} times as loud as recorded", replay_gain());
        }
        let Some(wav) = wav else {
            let _ = writeln!(log.stamp(), "sound: {} is not there, or no WAV file of 16-bit samples in one or two channels", path);
            return false;
        };
        let Some(&(_, codec_file)) = CODEC_FILES.iter().find(|(rate, _)| *rate == wav.rate) else {
            let _ = writeln!(log.stamp(), "sound: {} Hz; the codec has settings for 48000 and 44100 only", wav.rate);
            return false;
        };
        let mut live = self.up(codec_awake, true).await;
        let ready = chain(&mut live.codec, &mut live.select, bus, image, files, codec_file, wav.rate, volume, true, None, log).await;
        self.live = Some(live);
        if !ready {
            self.stop(codec_awake, bus, log);
            return false;
        }
        CUT.store(false, Ordering::Relaxed);
        DONE.reset();
        JOB.signal(match sound {
            Sound::File(path) => Job::File(image, path, wav),
            Sound::Recording => Job::Recording(wav),
        });
        true
    }

    /// Bring the codec up and have the player task record two seconds
    /// from the microphone into memory, as the stock app's microphone
    /// test does (0x080BE39A): the codec with the settings it has for
    /// playing at 48 kHz, which put the microphone on its second audio
    /// port, and nothing else; the amplifier stays off. True if the
    /// recording is on its way: then `stop` has to follow once `finished`
    /// says so.
    pub async fn record(&mut self, codec_awake: &mut Output<'static>, bus: &mut Bus, image: Partition, files: Partition, log: &mut Log) -> bool {
        if self.live.is_some() {
            return false;
        }
        let _ = writeln!(log.stamp(), "sound: recording from the microphone");
        let mut live = self.up(codec_awake, false).await;
        let ready = chain(&mut live.codec, &mut live.select, bus, image, files, CODEC_FILES[0].1, RECORD_RATE, VOLUME_QUIET, false, Some(INPUT_RECORDING), log).await;
        self.live = Some(live);
        if !ready {
            self.stop(codec_awake, bus, log);
            return false;
        }
        CUT.store(false, Ordering::Relaxed);
        DONE.reset();
        JOB.signal(Job::Record);
        true
    }

    /// Supplies, clock and buses on, in the stock app's order
    /// (0x080A9B88); `amplifier` says whether it will be started.
    async fn up(&mut self, codec_awake: &mut Output<'static>, amplifier: bool) -> Live {

        // Safety: the pins and peripherals are this driver's own, and
        // nothing made of them is alive (`live` is `None`).
        let r = &self.r;
        let (pg6, pg10, pb6, pb12) = unsafe { (r.amplifier_supply.clone_unchecked(), r.amplifier_reset.clone_unchecked(), r.codec_reset.clone_unchecked(), r.codec_select.clone_unchecked()) };
        let (spi2, pb10, pc1, pc2) = unsafe { (r.codec_spi.clone_unchecked(), r.codec_clock.clone_unchecked(), r.codec_out.clone_unchecked(), r.codec_in.clone_unchecked()) };
        let (tim2, pa3) = unsafe { (r.master_clock_timer.clone_unchecked(), r.master_clock.clone_unchecked()) };

        // Up, in the stock app's order (0x080A9B88).
        let mut amplifier_reset = Output::new(pg10, Level::Low, Speed::Low);
        let select = Output::new(pb12, Level::High, Speed::VeryHigh);
        let mut config = spi::Config::default();
        config.mode = spi::MODE_0;
        config.frequency = Hertz(7_500_000);
        let codec = Spi::new_blocking(spi2, pb10, pc1, pc2, config);
        let clock_pin = PwmPin::new(pa3, OutputType::PushPull);
        let mut clock = SimplePwm::new(tim2, None, None, None, Some(clock_pin), Hertz(3_000_000), Default::default());
        {
            let mut channel = clock.ch4();
            channel.set_duty_cycle(channel.max_duty_cycle() / 2);
            channel.enable();
        }
        codec_awake.set_high();
        let amplifier_supply = Output::new(pg6, Level::High, Speed::Low);
        let codec_reset = Output::new(pb6, Level::High, Speed::Low);
        amplifier_reset.set_high();
        Timer::after_millis(5).await;
        amplifier_reset.set_low();
        Timer::after_millis(15).await;
        watchdog::feed();
        Live { codec, select, _clock: clock, _amplifier_supply: amplifier_supply, _amplifier_reset: amplifier_reset, _codec_reset: codec_reset, amplifier }
    }

    /// Take codec and amplifier down again (0x080A9A78): the amplifier
    /// muted and powered down, then the supplies, the buses, the clock
    /// and the resets.
    pub fn stop(&mut self, codec_awake: &mut Output<'static>, bus: &mut Bus, log: &mut Log) {
        let Some(live) = self.live.take() else {
            return;
        };
        let amplifier = live.amplifier;
        let muted = amplifier && amplifier_change(bus, 0x08, 0, 1);
        let down = amplifier && amplifier_change(bus, 0x04, 0, 1);
        codec_awake.set_low();
        drop(live);
        watchdog::feed();
        if !amplifier {
            let _ = writeln!(log.stamp(), "sound: codec and clock off");
            return;
        }
        let _ = writeln!(
            log.stamp(),
            "sound: amplifier {} and {}, codec and clock off",
            if muted { "muted" } else { "NOT seen to mute" },
            if down { "powered down" } else { "NOT seen to power down" }
        );
    }
}

/// Codec set up for `rate` and, with `amplifier`, the amplifier started
/// and unmuted. Leaves the
/// amplifier as it is if a step fails; the caller takes everything down.
#[allow(clippy::too_many_arguments)]
async fn chain(codec: &mut CodecBus<'_>, select: &mut Output<'_>, bus: &mut Bus, image: Partition, files: Partition, codec_file: &str, rate: u32, volume: u32, amplifier: bool, input: Option<u32>, log: &mut Log) -> bool {
    let id = codec_read(codec, select, 0);
    if id != 0x6370 {
        let _ = writeln!(log.stamp(), "sound: the codec's register 0 reads {:#06x}, not 0x6370", id);
        return false;
    }
    // The codec's settings.
    let mut buffer = [0u8; 1024];
    let Some(settings) = read_file(image, codec_file, &mut buffer) else {
        let _ = writeln!(log.stamp(), "sound: {} could not be read", codec_file);
        return false;
    };
    let said = u32::from_be_bytes([settings[0], settings[1], settings[2], settings[3]]) as usize;
    if settings.len() < 12 || said + 4 != settings.len() || said % 8 != 0 {
        let _ = writeln!(log.stamp(), "sound: {} is {} bytes and says {}: not the file that was read", codec_file, settings.len(), said);
        return false;
    }
    let mut taken = true;
    for (n, record) in settings[4..].chunks_exact(8).enumerate() {
        let address = u32::from_be_bytes([record[0], record[1], record[2], record[3]]);
        let value = u32::from_be_bytes([record[4], record[5], record[6], record[7]]);
        // The microphone's gain, where another than the file's is wanted.
        let value = input.filter(|_| address == INPUT_REGISTER).unwrap_or(value);
        taken &= codec_write(codec, select, address, value);
        if n == 0 {
            // The first record resets the codec.
            let mut waited = 0;
            while codec_read(codec, select, 0x1880) & 0x0080 == 0 && waited < 50 {
                Timer::after_millis(1).await;
                waited += 1;
            }
        }
    }
    for register in [0x0701, 0x0703, 0x0709, 0x070B] {
        taken &= codec_write(codec, select, register, volume);
    }
    // The amplifier wants the bit clock for 5 ms before it is spoken to.
    Timer::after_millis(20).await;
    watchdog::feed();
    let status = codec_read(codec, select, 0x1881);
    let _ = writeln!(log.stamp(), "sound: codec settings {} for {} Hz, clock status {:#06x}, volume {:#06x}", if taken { "taken" } else { "NOT all taken" }, rate, status, codec_read(codec, select, 0x0703));
    if !taken || !amplifier {
        return taken;
    }

    // The amplifier, muted until the end.
    if amplifier_read(bus, 0x00) != Some(0x0310) {
        let _ = writeln!(log.stamp(), "sound: the amplifier does not answer with its ID");
        return false;
    }
    // Mono of both channels, the rate (8 = 48 kHz, 7 = 44.1 kHz), 16 bit,
    // 32 clocks a frame.
    let rate_code = if rate == 48_000 { 8 } else { 7 };
    let mut taken = amplifier_change(bus, 0x05, 0x000F | 3 << 4 | 3 << 6 | 3 << 10, rate_code | 3 << 10);
    for (register, value) in AMPLIFIER_TABLE {
        taken &= amplifier_write(bus, register, value);
    }
    if !taken || !pll_locked(bus).await {
        let _ = writeln!(log.stamp(), "sound: amplifier table {}, its PLL NOT locked (status {:#06x})", if taken { "taken" } else { "NOT taken" }, amplifier_read(bus, 0x01).unwrap_or(0));
        return false;
    }
    let version = {
        let mut word = |at: u16| if amplifier_write(bus, 0x40, at) { amplifier_read(bus, 0x41) } else { None };
        match (word(0x0F80), word(0x0F81)) {
            (Some(low), Some(high)) => (high as u32) << 16 | low as u32,
            _ => 1,
        }
    };
    let patch_file = match version {
        0 => PATCH_FILES[0],
        0xDEAC_97D6 => PATCH_FILES[1],
        _ => {
            let _ = writeln!(log.stamp(), "sound: amplifier ROM version {:#010x}: no patch is known for it", version);
            return false;
        }
    };
    let mut patch_buffer = [0u8; 512];
    let mut settings_buffer = [0u8; 128];
    let (Some(patch), Some(settings)) = (read_file(files, patch_file, &mut patch_buffer), read_file(files, SETTINGS_FILE, &mut settings_buffer)) else {
        let _ = writeln!(log.stamp(), "sound: {} or {} is not on the log volume (put them there with tools/ota.sh put)", patch_file, SETTINGS_FILE);
        return false;
    };
    if patch.len() % 2 != 0 || settings.len() != 124 {
        let _ = writeln!(log.stamp(), "sound: the amplifier's files have {} and {} bytes; an even number and 124 are expected", patch.len(), settings.len());
        return false;
    }
    // Muted, its processor out of the path, then the patch and the
    // settings into its memory (0x080D8E64 on).
    let mut loaded = amplifier_change(bus, 0x08, 0, 1) && amplifier_change(bus, 0x04, 0, 4);
    loaded = loaded && amplifier_load(bus, 0x8C00, patch) && amplifier_load(bus, 0x8380, settings);
    watchdog::feed();
    // The processor back in; a millisecond later its watchdog register
    // must count.
    loaded = loaded && amplifier_change(bus, 0x04, 4, 0);
    Timer::after_millis(1).await;
    let watchdog_counts = amplifier_read(bus, 0x42).unwrap_or(0);
    if !loaded || watchdog_counts == 0 {
        let _ = writeln!(log.stamp(), "sound: amplifier ROM {:#010x}, {} ({} bytes) and settings {}, watchdog register {:#06x}", version, patch_file, patch.len(), if loaded { "loaded" } else { "NOT loaded" }, watchdog_counts);
        // As the stock app leaves it then: processor out, muted.
        amplifier_change(bus, 0x04, 0, 4);
        return false;
    }
    // Its data output on (and for the older ROM a bit of its own), power
    // up, PLL locked, unmute (0x080D8BF0, 0x080D8B94).
    let mut ready = amplifier_change(bus, 0x06, 0, 1);
    if version == 0 {
        ready &= amplifier_change(bus, 0x20, 0, 0x8000);
    }
    ready &= amplifier_change(bus, 0x04, 1, 0);
    Timer::after_millis(2).await;
    if !ready || !pll_locked(bus).await {
        let _ = writeln!(log.stamp(), "sound: the amplifier did not come up (status {:#06x})", amplifier_read(bus, 0x01).unwrap_or(0));
        return false;
    }
    // Read its three status registers, which clears what was pending, and
    // let it report clock and supply trouble (0x080D8A0E, 0x080D89A4).
    let status = amplifier_read(bus, 0x01).unwrap_or(0);
    let _ = (amplifier_read(bus, 0x02), amplifier_read(bus, 0x03));
    amplifier_change(bus, 0x03, 0x4073, 0);
    let unmuted = amplifier_change(bus, 0x08, 1, 0);
    let _ = writeln!(log.stamp(), "sound: amplifier patched ({}), up with status {:#06x}, {}", patch_file, status, if unmuted { "unmuted" } else { "NOT unmuted" });
    unmuted
}

/// The task that sends a sound's samples out of SAI1: it waits for a
/// file from `Audio::start`, sends it with a little silence before and
/// after, and says how it went.
#[embassy_executor::task]
pub async fn player(port: Port) -> ! {
    static mut RING: [u16; 8192] = [0; 8192];
    // Safety: there is one player task, and nothing else uses this.
    let ring = unsafe { &mut *core::ptr::addr_of_mut!(RING) };
    loop {
        let outcome = match JOB.wait().await {
            Job::File(volume, path, wav) => send(&port, ring, Some((volume, path)), &wav).await,
            Job::Recording(wav) => send(&port, ring, None, &wav).await,
            Job::Record => record(&port, ring).await,
        };
        DONE.signal(outcome);
        // The main task may be waiting for something else.
        crate::hal::wake::nudge();
    }
}

/// The port's frame, the same for both directions: two slots of 16 bits
/// in 32 clocks, the frame signal low first and a bit early, the clocks
/// the codec's.
fn frame(direction: sai::TxRx, stereo_mono: sai::StereoMono, threshold: sai::FifoThreshold) -> sai::Config {
    let mut config = sai::Config::default();
    config.mode = sai::Mode::Slave;
    config.tx_rx = direction;
    config.protocol = sai::Protocol::Free;
    config.slot_size = sai::SlotSize::Channel16;
    config.data_size = sai::DataSize::Data16;
    config.stereo_mono = stereo_mono;
    config.bit_order = sai::BitOrder::MsbFirst;
    config.frame_sync_offset = sai::FrameSyncOffset::BeforeFirstBit;
    config.frame_sync_polarity = sai::FrameSyncPolarity::ActiveLow;
    config.frame_sync_definition = sai::FrameSyncDefinition::ChannelIdentification;
    config.frame_length = 32;
    // Data changes on the falling edge of the clock and is taken on the
    // rising one.
    config.clock_strobe = sai::ClockStrobe::Rising;
    config.fifo_threshold = threshold;
    config
}

/// Record from the microphone into memory. The codec sends its samples
/// in both slots of the frame; block B of the port, which goes by block
/// A's clock and frame pins, keeps the first (mono mode), as the stock
/// app has it (0x0809025A). Block B is started first and block A, which
/// sends nothing, after it: the order of the stock app's loop test
/// (0x080BE81C); the HAL switches a block on when it is made.
async fn record(port: &Port, ring: &mut [u16]) -> Outcome {
    let mut outcome = Outcome { sent: 0, dry: 0, failed: false, cut: false, recorded: true, peak: 0, level: 0 };
    RECORDED.store(0, Ordering::Relaxed);
    static mut IDLE: [u16; 64] = [0; 64];
    // Safety: the port and these buffers are this task's own, and what is
    // made of them here is gone again when this returns.
    let idle = unsafe { &mut *core::ptr::addr_of_mut!(IDLE) };
    let memory = unsafe { recording() };
    let (sai1, clock, frame_pin, data, channel) = unsafe { (port.port.clone_unchecked(), port.clock.clone_unchecked(), port.frame.clone_unchecked(), port.data.clone_unchecked(), port.dma.clone_unchecked()) };
    let (input, input_channel) = unsafe { (port.input.clone_unchecked(), port.input_dma.clone_unchecked()) };
    let (block_a, block_b) = sai::split_subblocks(sai1);
    let mut receiver: Sai<'_, u16> = Sai::new_synchronous(block_b, input, input_channel, Irqs, ring, frame(sai::TxRx::Receiver, sai::StereoMono::Mono, sai::FifoThreshold::ThreeQuarters));
    receiver.start();
    let _sender: Sai<'_, u16> = Sai::new_asynchronous(block_a, clock, data, frame_pin, channel, Irqs, idle, frame(sai::TxRx::Transmitter, sai::StereoMono::Mono, sai::FifoThreshold::Quarter));

    let mut piece = [0u16; 1024];
    let mut skip = RECORD_SKIP;
    let mut kept = 0usize;
    let mut sum = 0u64;
    while kept < memory.len() {
        if CUT.load(Ordering::Relaxed) {
            outcome.cut = true;
            break;
        }
        // Without the codec's clocks nothing arrives: half a second is
        // the most a piece gets. Samples lost because they were not
        // fetched in time are an error of the read, counted.
        match with_timeout(Duration::from_millis(500), receiver.read(&mut piece)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                outcome.dry += 1;
                if outcome.dry > 20 {
                    outcome.failed = true;
                    break;
                }
                continue;
            }
            Err(_) => {
                outcome.failed = true;
                break;
            }
        }
        let fresh = &piece[skip.min(piece.len())..];
        skip = skip.saturating_sub(piece.len());
        let n = fresh.len().min(memory.len() - kept);
        for (to, from) in memory[kept..kept + n].iter_mut().zip(fresh) {
            let sample = *from as i16;
            *to = sample;
            outcome.peak = outcome.peak.max(sample.unsigned_abs());
            sum += sample.unsigned_abs() as u64;
        }
        kept += n;
        watchdog::feed();
    }
    drop(receiver);
    outcome.sent = kept as u32;
    outcome.level = if kept > 0 { (sum / kept as u64) as u16 } else { 0 };
    if !outcome.failed {
        RECORDED_PEAK.store(outcome.peak as u32, Ordering::Relaxed);
        RECORDED.store(kept as u32, Ordering::Relaxed);
    }
    outcome
}

/// Send a sound's samples: a file's, or with no file the recording's.
async fn send(port: &Port, ring: &mut [u16], file: Option<(Partition, &'static str)>, wav: &Wav) -> Outcome {
    let mut outcome = Outcome { sent: 0, dry: 0, failed: false, cut: false, recorded: false, peak: 0, level: 0 };
    // Safety: the port is this task's own, and what is made of it here is
    // gone again when this returns.
    let (sai1, clock, frame_pin, data, channel) = unsafe { (port.port.clone_unchecked(), port.clock.clone_unchecked(), port.frame.clone_unchecked(), port.data.clone_unchecked(), port.dma.clone_unchecked()) };
    let (block, _) = sai::split_subblocks(sai1);
    // A one-channel sound goes out on both slots.
    let config = frame(sai::TxRx::Transmitter, if wav.channels == 1 { sai::StereoMono::Mono } else { sai::StereoMono::Stereo }, sai::FifoThreshold::Quarter);
    let mut out: Sai<'_, u16> = Sai::new_asynchronous(block, clock, data, frame_pin, channel, Irqs, ring, config);

    let mut reader = match file {
        Some((volume, path)) => match storage::FileReader::open(volume, path) {
            Ok(reader) => Some(reader),
            Err(_) => {
                outcome.failed = true;
                return outcome;
            }
        },
        None => None,
    };
    // Safety: the recording is this task's own.
    let memory = unsafe { recording() };
    let mut bytes = [0u8; 4096];
    let mut samples = [0u16; 2048];
    // Past the file's head, to where the samples begin.
    let mut skip = wav.data_at as usize;
    while let Some(reader) = reader.as_mut().filter(|_| skip > 0 && !outcome.failed) {
        let n = skip.min(bytes.len());
        outcome.failed = reader.read(&mut bytes[..n]).unwrap_or(0) != n;
        skip -= n;
    }
    // A piece into the buffer; false if it could not be got in. Without
    // the codec's clocks nothing leaves the buffer, and a write would wait
    // for ever: half a second is the most it gets. The HAL reports a port
    // that ran dry as an error of the next write and starts afresh: a gap
    // in the sound, counted, and no reason to give up.
    async fn push(out: &mut Sai<'_, u16>, piece: &[u16], dry: &mut u32) -> bool {
        for _ in 0..3 {
            match with_timeout(Duration::from_millis(500), out.write(piece)).await {
                Ok(Ok(())) => return true,
                Ok(Err(_)) => *dry += 1,
                Err(_) => return false,
            }
        }
        false
    }
    // The buffer has to hold something before the port is started (the
    // HAL's rule for sending): a little silence.
    outcome.failed |= !push(&mut out, &[0u16; 2048], &mut outcome.dry).await;
    out.start();
    let mut left = wav.data_len as usize;
    while left > 0 && !outcome.failed {
        if CUT.load(Ordering::Relaxed) {
            outcome.cut = true;
            break;
        }
        let want = left.min(bytes.len());
        let n = match reader.as_mut() {
            Some(reader) => {
                let n = reader.read(&mut bytes[..want]).unwrap_or(0) / 2;
                for (sample, pair) in samples.iter_mut().zip(bytes.chunks_exact(2)) {
                    *sample = u16::from_le_bytes([pair[0], pair[1]]);
                }
                n
            }
            None => {
                let at = outcome.sent as usize;
                let n = (want / 2).min(memory.len().saturating_sub(at));
                let gain = replay_gain() as i32;
                for (sample, from) in samples.iter_mut().zip(&memory[at..at + n]) {
                    *sample = (*from as i32 * gain).clamp(-32768, 32767) as i16 as u16;
                }
                n
            }
        };
        if n == 0 {
            break;
        }
        outcome.failed |= !push(&mut out, &samples[..n], &mut outcome.dry).await;
        outcome.sent += n as u32;
        left -= n * 2;
        watchdog::feed();
    }
    if let Some(reader) = reader {
        reader.close();
    }
    // Silence until what was sent has left the buffer.
    for _ in 0..5 {
        if outcome.failed {
            break;
        }
        outcome.failed |= !push(&mut out, &[0u16; 2048], &mut outcome.dry).await;
    }
    drop(out);
    outcome
}

/// Wait for the amplifier's PLL to lock to the codec's bit clock: five
/// looks, 2 ms apart.
async fn pll_locked(bus: &mut Bus) -> bool {
    for _ in 0..5 {
        if amplifier_read(bus, 0x01).is_some_and(|status| status & 1 != 0) {
            return true;
        }
        Timer::after_millis(2).await;
    }
    false
}
