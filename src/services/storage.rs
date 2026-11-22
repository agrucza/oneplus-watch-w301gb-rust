//! FAT filesystem on the eMMC.
//!
//! The card carries an ordinary partition table with four FAT partitions,
//! which the stock firmware mounts as FatFs volumes `0:` to `3:`
//! (0 System, 1 User incl. `packetA`/`ota`, 2 data, 3 logs and music).
//! Here they are `VolumeIdx(0)` to `VolumeIdx(3)`.
//!
//! A device is read-only unless opened for writing, and even then refuses
//! any sector outside its own partition.

use core::fmt;

use core::ops::ControlFlow;

use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, LfnBuffer, Mode, RawDirectory, RawFile, RawVolume, ShortFileName,
    TimeSource, Timestamp, VolumeIdx, VolumeManager,
};

use crate::hal::{rtc, sdmmc};

/// Block-device error: an eMMC driver error, or a refused write.
#[derive(Debug, Clone, Copy)]
pub enum DeviceError {
    Emmc(u8),
    ReadOnly,
    /// A write outside the partition this device was opened for.
    OutOfBounds,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::Emmc(code) => write!(f, "eMMC error {}", code),
            DeviceError::ReadOnly => f.write_str("storage is read-only"),
            DeviceError::OutOfBounds => f.write_str("write outside the partition"),
        }
    }
}

impl core::error::Error for DeviceError {}

/// Diagnosis: how the filesystem crate writes. Sector writes in total, how
/// many of them were to the sector right after the previous write, and the
/// longest such run.
pub struct WriteStats {
    pub sectors: u32,
    pub in_sequence: u32,
    pub longest_run: u32,
}

static WRITES: [core::sync::atomic::AtomicU32; 5] = [const { core::sync::atomic::AtomicU32::new(0) }; 5];

/// The write statistics since the last call; the call starts them afresh.
pub fn take_write_stats() -> WriteStats {
    use core::sync::atomic::Ordering::Relaxed;
    let stats = WriteStats { sectors: WRITES[0].swap(0, Relaxed), in_sequence: WRITES[1].swap(0, Relaxed), longest_run: WRITES[2].swap(0, Relaxed) };
    WRITES[3].store(0, Relaxed);
    WRITES[4].store(u32::MAX - 1, Relaxed);
    stats
}

fn note_write(sector: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    WRITES[0].fetch_add(1, Relaxed);
    let run = if sector == WRITES[4].load(Relaxed).wrapping_add(1) {
        WRITES[1].fetch_add(1, Relaxed);
        WRITES[3].load(Relaxed) + 1
    } else {
        1
    };
    WRITES[3].store(run, Relaxed);
    WRITES[2].fetch_max(run, Relaxed);
    WRITES[4].store(sector, Relaxed);
}

/// Sector writes collected to go out with one command.
///
/// The filesystem crate writes one sector per call. Measured on a 256 KB
/// file: 530 sector writes, 504 of them to the sector right after the one
/// before, in runs of up to 64 (one cluster), with a few writes to the
/// allocation table in between. So consecutive writes are collected here
/// and sent together (`sdmmc::write_sectors`).
///
/// What is collected goes to the card when a write is not the next sector,
/// when the run is full, when one of its sectors is asked for, and before
/// any of this module's writing functions returns (`flush_writes`). So
/// nothing stays in memory once a caller has its answer.
struct Collected {
    first: u32,
    /// Sectors waiting in `data`; 0 = nothing.
    count: u32,
    data: [u32; COLLECT * 128],
}

const COLLECT: usize = 64;

struct SharedCollected(core::cell::UnsafeCell<Collected>);
// Safety: only touched from thread mode, by the one executor thread.
unsafe impl Sync for SharedCollected {}
static COLLECTED: SharedCollected = SharedCollected(core::cell::UnsafeCell::new(Collected { first: 0, count: 0, data: [0; COLLECT * 128] }));
/// Diagnosis and safety valve: false sends every sector on its own.
static COLLECT_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Switch collecting writes on or off; what is waiting goes out first.
pub fn set_write_collecting(on: bool) {
    let _ = flush_writes();
    COLLECT_ON.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// Send what is collected to the card.
pub fn flush_writes() -> Result<(), DeviceError> {
    // Safety: see `SharedCollected`.
    let c = unsafe { &mut *COLLECTED.0.get() };
    let count = core::mem::take(&mut c.count) as usize;
    if count == 0 {
        return Ok(());
    }
    let result = sdmmc::write_sectors(c.first, &c.data[..count * 128]);
    crate::boot::watchdog::feed();
    let Err(first_error) = result else {
        return Ok(());
    };
    if count == 1 {
        return Err(DeviceError::Emmc(first_error as u8));
    }
    // The run failed as a whole: get the card out of the transfer, do
    // without collecting from here on, and write the sectors one by one.
    COLLECT_ON.store(false, core::sync::atomic::Ordering::Relaxed);
    sdmmc::stop_transfer();
    for i in 0..count {
        sdmmc::write_sectors(c.first + i as u32, &c.data[i * 128..][..128]).map_err(|e| DeviceError::Emmc(e as u8))?;
        crate::boot::watchdog::feed();
    }
    Ok(())
}

/// Take one sector for writing: collect it, or send it at once.
fn write_collected(sector: u32, contents: &[u8; 512]) -> Result<(), DeviceError> {
    // Safety: see `SharedCollected`.
    let c = unsafe { &mut *COLLECTED.0.get() };
    let fits = c.count != 0 && sector == c.first + c.count && (c.count as usize) < COLLECT;
    if !fits {
        flush_writes()?;
        c.first = sector;
    }
    let slot = &mut c.data[c.count as usize * 128..][..128];
    for (word, bytes) in slot.iter_mut().zip(contents.chunks_exact(4)) {
        *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    c.count += 1;
    if !COLLECT_ON.load(core::sync::atomic::Ordering::Relaxed) {
        flush_writes()?;
    }
    Ok(())
}

/// True if `sector` is waiting to be written.
fn is_collected(sector: u32) -> bool {
    // Safety: see `SharedCollected`.
    let c = unsafe { &*COLLECTED.0.get() };
    c.count != 0 && sector >= c.first && sector < c.first + c.count
}

/// Sectors fetched with one command when a file is read front to back.
const READ_AHEAD: usize = 64;

/// The last run of sectors fetched ahead, and what is needed to notice
/// reading in sequence.
///
/// The filesystem crate asks for one sector at a time, also when it reads
/// a file from front to back, and in between it re-reads the allocation
/// table. One command per sector costs about 0.15 ms each; a run of 64
/// sectors with one command costs 2 ms (measured: 1 MB in 312 ms against
/// 64 ms). So: a request for the sector right after the previous request,
/// or right after the run, fetches a whole run; any other request (the
/// allocation table, directories) is read on its own and leaves the run
/// alone.
struct ReadAhead {
    first: u32,
    /// Sectors in `data` that are valid; 0 = nothing.
    count: u32,
    /// The sector asked for last.
    last: u32,
    data: [u32; READ_AHEAD * 128],
}

struct SharedReadAhead(core::cell::UnsafeCell<ReadAhead>);
// Safety: only touched from thread mode, by the one executor thread, and
// never across an await.
unsafe impl Sync for SharedReadAhead {}
static AHEAD: SharedReadAhead =
    SharedReadAhead(core::cell::UnsafeCell::new(ReadAhead { first: 0, count: 0, last: 0, data: [0; READ_AHEAD * 128] })); // all zero: takes no room in the image // all zero: takes no room in the image
/// Diagnosis and safety valve: false switches reading ahead off.
static AHEAD_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Switch reading ahead on or off (off also forgets what was fetched).
pub fn set_read_ahead(on: bool) {
    AHEAD_ON.store(on, core::sync::atomic::Ordering::Relaxed);
    forget_read_ahead();
}

/// Forget what was fetched ahead; called after every write.
fn forget_read_ahead() {
    // Safety: see `SharedReadAhead`.
    unsafe { (*AHEAD.0.get()).count = 0 };
}

/// Read `sector`, which lies in a partition that ends before `end`, using
/// and filling the read-ahead run.
fn read_cached(sector: u32, end: u32, out: &mut [u8; 512]) -> Result<(), DeviceError> {
    // Safety: see `SharedReadAhead`.
    let a = unsafe { &mut *AHEAD.0.get() };
    let in_sequence = sector == a.last.wrapping_add(1) || (a.count != 0 && sector == a.first + a.count);
    a.last = sector;
    if !AHEAD_ON.load(core::sync::atomic::Ordering::Relaxed) {
        return read_raw(sector, out);
    }
    if a.count == 0 || sector < a.first || sector >= a.first + a.count {
        let run = (end.saturating_sub(sector) as usize).min(READ_AHEAD);
        if !in_sequence || run < 2 {
            return read_raw(sector, out);
        }
        a.count = 0;
        if sdmmc::read_sectors(sector, &mut a.data[..run * 128]).is_err() {
            // Do without reading ahead from here on, and try the plain way.
            AHEAD_ON.store(false, core::sync::atomic::Ordering::Relaxed);
            return read_raw(sector, out);
        }
        a.first = sector;
        a.count = run as u32;
        crate::boot::watchdog::feed();
    }
    let words = &a.data[(sector - a.first) as usize * 128..][..128];
    for (bytes, word) in out.chunks_exact_mut(4).zip(words) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    Ok(())
}

fn read_raw(sector: u32, out: &mut [u8; 512]) -> Result<(), DeviceError> {
    let mut aligned = [0u32; 128]; // the controller's DMA needs a word-aligned buffer
    sdmmc::read_sector(sector, &mut aligned).map_err(|e| DeviceError::Emmc(e as u8))?;
    for (bytes, word) in out.chunks_exact_mut(4).zip(aligned.iter()) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    crate::boot::watchdog::feed();
    Ok(())
}

/// Filesystem found in a partition's boot sector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Fat16 = 1,
    Fat32 = 2,
    /// FAT12, exFAT or not a FAT boot sector at all.
    Other = 3,
}

/// One entry of the card's partition table, plus what its boot sector says.
#[derive(Clone, Copy, Debug)]
pub struct Partition {
    pub table_type: u8,
    pub start: u32,
    pub sectors: u32,
    pub kind: FsKind,
}

/// Look at partition `index` (0..=3) the way FatFs does: ignore the type byte
/// in the partition table and classify by the boot sector's own parameters.
pub fn inspect(index: usize) -> Result<Partition, DeviceError> {
    let mut s = [0u8; 512];
    read_raw(0, &mut s)?;
    let e = &s[0x1BE + 16 * index..][..16];
    let table_type = e[4];
    let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
    let sectors = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);

    read_raw(start, &mut s)?;
    let u16_at = |i: usize| u16::from_le_bytes([s[i], s[i + 1]]) as u32;
    let u32_at = |i: usize| u32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]);
    let bytes_per_sector = u16_at(11);
    let sectors_per_cluster = s[13] as u32;
    let kind = if s[510] != 0x55 || s[511] != 0xAA || bytes_per_sector != 512 || sectors_per_cluster == 0 {
        FsKind::Other // includes exFAT, whose classic parameter block is all zero
    } else {
        // Microsoft's rule: the FAT type follows from the cluster count alone.
        let fat_size = if u16_at(22) != 0 { u16_at(22) } else { u32_at(36) };
        let total = if u16_at(19) != 0 { u16_at(19) } else { u32_at(32) };
        let root_dir = (u16_at(17) * 32).div_ceil(512);
        let meta = u16_at(14) + s[16] as u32 * fat_size + root_dir;
        match total.saturating_sub(meta) / sectors_per_cluster {
            0..=4084 => FsKind::Other, // FAT12
            4085..=65524 => FsKind::Fat16,
            _ => FsKind::Fat32,
        }
    };
    Ok(Partition { table_type, start, sectors, kind })
}

/// One partition of the eMMC as a block device for the filesystem crate.
///
/// The crate insists on finding the volume through a partition table with a
/// type byte it knows. FatFs never looks at that byte, so the stock card may
/// carry anything there. This device therefore answers sector 0 with a
/// one-entry table of its own (correct type, real position); every other
/// sector is the card's. `sdmmc::init()` must have succeeded.
pub struct Emmc {
    partition: Partition,
    writable: bool,
}

/// A last line of defence: when a writable device goes away, whatever is
/// still collected goes to the card.
impl Drop for Emmc {
    fn drop(&mut self) {
        if self.writable {
            let _ = flush_writes();
        }
    }
}

impl BlockDevice for Emmc {
    type Error = DeviceError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), DeviceError> {
        for (i, block) in blocks.iter_mut().enumerate() {
            let sector = start.0 + i as u32;
            if sector == 0 {
                let p = &self.partition;
                block.contents = [0; 512];
                let e = &mut block.contents[0x1BE..0x1CE];
                e[4] = if p.kind == FsKind::Fat16 { 0x06 } else { 0x0C };
                e[8..12].copy_from_slice(&p.start.to_le_bytes());
                e[12..16].copy_from_slice(&p.sectors.to_le_bytes());
                block.contents[510] = 0x55;
                block.contents[511] = 0xAA;
            } else {
                if is_collected(sector) {
                    flush_writes()?; // the card must have it before it is read
                }
                let p = &self.partition;
                let end = p.start.saturating_add(p.sectors);
                if sector >= p.start && sector < end {
                    read_cached(sector, end, &mut block.contents)?;
                } else {
                    read_raw(sector, &mut block.contents)?;
                }
            }
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), DeviceError> {
        if !self.writable {
            return Err(DeviceError::ReadOnly);
        }
        let p = &self.partition;
        for (i, block) in blocks.iter().enumerate() {
            let sector = start.0 + i as u32;
            // Never the partition table, never another partition.
            if sector < p.start || sector - p.start >= p.sectors || p.start == 0 {
                return Err(DeviceError::OutOfBounds);
            }
            forget_read_ahead();
            note_write(sector);
            write_collected(sector, &block.contents)?;
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, DeviceError> {
        // The card's own figure once it has been read; until then no limit.
        // Not if it would cut into this partition: then the figure is not
        // to be trusted.
        let end = self.partition.start.saturating_add(self.partition.sectors);
        Ok(BlockCount(match sdmmc::sectors() {
            n if n != 0 && n >= end => n,
            _ => u32::MAX,
        }))
    }
}

/// File timestamps: fixed until the RTC is wired in.
pub struct FileClock;

impl TimeSource for FileClock {
    /// The time written to files: the real-time clock's, or 2026-01-01 if
    /// that was never set.
    fn get_timestamp(&self) -> Timestamp {
        match rtc::now() {
            Some(t) => Timestamp {
                year_since_1970: (t.year - 1970) as u8,
                zero_indexed_month: t.month - 1,
                zero_indexed_day: t.day - 1,
                hours: t.hour,
                minutes: t.minute,
                seconds: t.second,
            },
            None => Timestamp { year_since_1970: 56, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 },
        }
    }
}

/// Why `read_file` failed: which step (1..=6), and whether the reason was
/// simply "not found".
pub struct ReadError {
    pub step: u8,
    pub not_found: bool,
}

/// Read the start of `/dir1/dir2/file` on `partition` into `buf`. Names are the
/// short 8.3 names in upper case. Returns the number of bytes read.
pub fn read_file(partition: Partition, dir1: &str, dir2: &str, file: &str, buf: &mut [u8]) -> Result<usize, ReadError> {
    fn at<T>(step: u8, r: Result<T, embedded_sdmmc::Error<DeviceError>>) -> Result<T, ReadError> {
        r.map_err(|e| ReadError { step, not_found: matches!(e, embedded_sdmmc::Error::NotFound) })
    }
    let manager = VolumeManager::new(Emmc { partition, writable: false }, FileClock);
    let volume = at(1, manager.open_volume(VolumeIdx(0)))?;
    let root = at(2, volume.open_root_dir())?;
    let d1 = at(3, root.open_dir(dir1))?;
    let d2 = at(4, d1.open_dir(dir2))?;
    let f = at(5, d2.open_file_in_dir(file, Mode::ReadOnly))?;
    at(6, f.read(buf))
}

/// Append `data` to `/dir/file` on `partition`, creating the file if needed.
/// On failure returns the step: 2 volume, 3 root, 4 directory, 5 open,
/// 6 write, 7 close.
pub fn append(partition: Partition, dir: &str, file: &str, data: &[u8]) -> Result<(), u8> {
    let manager = VolumeManager::new(Emmc { partition, writable: true }, FileClock);
    let volume = manager.open_volume(VolumeIdx(0)).map_err(|_| 2)?;
    let root = volume.open_root_dir().map_err(|_| 3)?;
    let d = root.open_dir(dir).map_err(|_| 4)?;
    let f = d.open_file_in_dir(file, Mode::ReadWriteCreateOrAppend).map_err(|_| 5)?;
    let written = (|| {
        f.write(data).map_err(|_| 6)?;
        f.close().map_err(|_| 7)?;
        d.close().map_err(|_| 7)?;
        root.close().map_err(|_| 7)?;
        volume.close().map_err(|_| 7)
    })();
    // Whatever happened: nothing may stay collected in memory.
    let flushed = flush_writes().map_err(|_| 6);
    written.and(flushed)
}

/// Read the last `buf.len()` bytes (or all, if shorter) of `/dir/file`.
/// Returns (file length, bytes read).
pub fn read_tail(partition: Partition, dir: &str, file: &str, buf: &mut [u8]) -> Result<(u32, usize), u8> {
    let manager = VolumeManager::new(Emmc { partition, writable: false }, FileClock);
    let volume = manager.open_volume(VolumeIdx(0)).map_err(|_| 2)?;
    let root = volume.open_root_dir().map_err(|_| 3)?;
    let d = root.open_dir(dir).map_err(|_| 4)?;
    let f = d.open_file_in_dir(file, Mode::ReadOnly).map_err(|_| 5)?;
    let len = f.length();
    f.seek_from_start(len.saturating_sub(buf.len() as u32)).map_err(|_| 6)?;
    let n = f.read(buf).map_err(|_| 6)?;
    Ok((len, n))
}

/// Read a file in the root directory of `partition`, found by its long name
/// (compared without regard to case). Returns the file's length; `buf`
/// receives at most `buf.len()` bytes of it.
/// On failure returns the step: 2 volume, 3 root, 4 not found, 5 open, 6 read.
pub fn read_root_file(partition: Partition, long_name: &str, buf: &mut [u8]) -> Result<usize, u8> {
    let manager = VolumeManager::new(Emmc { partition, writable: false }, FileClock);
    let volume = manager.open_volume(VolumeIdx(0)).map_err(|_| 2)?;
    let root = volume.open_root_dir().map_err(|_| 3)?;

    // The filesystem crate opens files by their short 8.3 name only, so look
    // up which short name belongs to the long one.
    let mut storage = [0u8; 128];
    let mut lfn = LfnBuffer::new(&mut storage);
    let mut short: Option<ShortFileName> = None;
    root.iterate_dir_lfn(&mut lfn, |entry, long| {
        if long.is_some_and(|l| l.eq_ignore_ascii_case(long_name)) {
            short = Some(entry.name.clone());
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .map_err(|_| 3)?;
    let short = short.ok_or(4)?;

    let f = root.open_file_in_dir(&short, Mode::ReadOnly).map_err(|_| 5)?;
    let len = f.length() as usize;
    let mut done = 0;
    while done < len.min(buf.len()) {
        let n = f.read(&mut buf[done..]).map_err(|_| 6)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(len)
}

/// An existing file opened for rewriting from the start, kept open across
/// many `write` calls (used for receiving a firmware image piece by piece).
///
/// The file must already exist: the filesystem crate cannot create long
/// file names, and the bootloader looks for the long name.
pub struct FileWriter {
    manager: VolumeManager<Emmc, FileClock>,
    volume: RawVolume,
    file: RawFile,
}

impl FileWriter {
    /// Open `/dirs.../long_name` on `partition` and truncate it. Directory
    /// names are short 8.3 names in upper case; the file is found by its
    /// long or its short name (case-insensitive).
    /// On failure returns the step: 2 volume, 3 root, 4 directory,
    /// 5 file not found, 6 open.
    pub fn rewrite(partition: Partition, dirs: &[&str], long_name: &str) -> Result<Self, u8> {
        let manager = VolumeManager::new(Emmc { partition, writable: true }, FileClock);
        let volume = manager.open_raw_volume(VolumeIdx(0)).map_err(|_| 2)?;
        let mut dir: RawDirectory = manager.open_root_dir(volume).map_err(|_| 3)?;
        for name in dirs {
            let next = manager.open_dir(dir, *name).map_err(|_| 4)?;
            let _ = manager.close_dir(dir);
            dir = next;
        }
        let short = match lookup(&manager, dir, long_name).map_err(|_| 4)? {
            Some((short, false)) => short,
            _ => return Err(5),
        };
        let file = manager.open_file_in_dir(dir, &short, Mode::ReadWriteTruncate).map_err(|_| 6)?;
        let _ = manager.close_dir(dir);
        Ok(Self { manager, volume, file })
    }

    /// Append `data` to the file.
    pub fn write(&mut self, data: &[u8]) -> Result<(), ()> {
        let written = self.manager.write(self.file, data).map_err(|_| ());
        // Nothing stays collected in memory once the caller has its answer.
        let flushed = flush_writes().map_err(|_| ());
        written.and(flushed)
    }

    /// Close the file, which also stores its final length.
    pub fn finish(self) -> Result<(), ()> {
        let closed = self.manager.close_file(self.file);
        let _ = self.manager.close_volume(self.volume);
        let flushed = flush_writes().map_err(|_| ());
        closed.map_err(|_| ()).and(flushed)
    }
}

/// Find `name` in an open directory, by long name or by short 8.3 name,
/// without regard to case. Returns the entry's short name (which is what
/// the filesystem crate opens things by) and whether it is a directory.
fn lookup(
    manager: &VolumeManager<Emmc, FileClock>,
    dir: RawDirectory,
    name: &str,
) -> Result<Option<(ShortFileName, bool)>, ()> {
    let mut storage = [0u8; 128];
    let mut lfn = LfnBuffer::new(&mut storage);
    let mut found = None;
    manager
        .iterate_dir_lfn(dir, &mut lfn, |entry, long| {
            let short = &entry.name;
            let (base, ext) = (short.base_name(), short.extension());
            let short_matches = match name.rsplit_once('.') {
                Some((b, e)) if !ext.is_empty() => {
                    b.as_bytes().eq_ignore_ascii_case(base) && e.as_bytes().eq_ignore_ascii_case(ext)
                }
                _ => ext.is_empty() && name.as_bytes().eq_ignore_ascii_case(base),
            };
            if short_matches || long.is_some_and(|l| l.eq_ignore_ascii_case(name)) {
                found = Some((short.clone(), entry.attributes.is_directory()));
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .map_err(|_| ())?;
    Ok(found)
}

/// A file opened for reading from the start, kept open across many `read`
/// calls (used for sending a file to the PC piece by piece).
pub struct FileReader {
    manager: VolumeManager<Emmc, FileClock>,
    volume: RawVolume,
    file: RawFile,
    len: u32,
}

impl FileReader {
    /// Open `path` (components separated by `/`, long or short names) on
    /// `partition`.
    /// On failure returns the step: 2 volume, 3 root, 4 not found, 5 open.
    pub fn open(partition: Partition, path: &str) -> Result<Self, u8> {
        let manager = VolumeManager::new(Emmc { partition, writable: false }, FileClock);
        let volume = manager.open_raw_volume(VolumeIdx(0)).map_err(|_| 2)?;
        let mut dir = manager.open_root_dir(volume).map_err(|_| 3)?;
        let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
        while let Some(part) = parts.next() {
            let (short, is_dir) = lookup(&manager, dir, part).map_err(|_| 3)?.ok_or(4)?;
            if parts.peek().is_some() {
                if !is_dir {
                    return Err(4);
                }
                let next = manager.open_dir(dir, &short).map_err(|_| 4)?;
                let _ = manager.close_dir(dir);
                dir = next;
            } else {
                if is_dir {
                    return Err(4);
                }
                let file = manager.open_file_in_dir(dir, &short, Mode::ReadOnly).map_err(|_| 5)?;
                let len = manager.file_length(file).map_err(|_| 5)?;
                let _ = manager.close_dir(dir);
                return Ok(Self { manager, volume, file, len });
            }
        }
        Err(4) // empty path
    }

    /// Length of the file in bytes.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Read the next bytes; returns how many were read (0 at the end).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut done = 0;
        while done < buf.len() {
            if self.manager.file_eof(self.file).map_err(|_| ())? {
                break;
            }
            let n = self.manager.read(self.file, &mut buf[done..]).map_err(|_| ())?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    pub fn close(self) {
        let _ = self.manager.close_file(self.file);
        let _ = self.manager.close_volume(self.volume);
    }
}

/// True if the file at `path` on `partition` starts with exactly the bytes
/// in `expected` (at most 512 are compared).
pub fn file_starts_with(partition: Partition, path: &str, expected: &[u8]) -> bool {
    let Ok(mut reader) = FileReader::open(partition, path) else {
        return false;
    };
    let mut buf = [0u8; 512];
    let n = expected.len().min(buf.len());
    let same = matches!(reader.read(&mut buf[..n]), Ok(got) if got == n) && buf[..n] == expected[..n];
    reader.close();
    same
}
