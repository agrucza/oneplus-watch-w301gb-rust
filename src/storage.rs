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

use crate::emmc;

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

fn read_raw(sector: u32, out: &mut [u8; 512]) -> Result<(), DeviceError> {
    let mut aligned = [0u32; 128]; // the controller's DMA needs a word-aligned buffer
    emmc::read_sector(sector, &mut aligned).map_err(|e| DeviceError::Emmc(e as u8))?;
    for (bytes, word) in out.chunks_exact_mut(4).zip(aligned.iter()) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    crate::watchdog::feed();
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
/// sector is the card's. `emmc::init()` must have succeeded.
pub struct Emmc {
    partition: Partition,
    writable: bool,
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
                read_raw(sector, &mut block.contents)?;
            }
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), DeviceError> {
        if !self.writable {
            return Err(DeviceError::ReadOnly);
        }
        let p = &self.partition;
        let mut aligned = [0u32; 128];
        for (i, block) in blocks.iter().enumerate() {
            let sector = start.0 + i as u32;
            // Never the partition table, never another partition.
            if sector < p.start || sector - p.start >= p.sectors || p.start == 0 {
                return Err(DeviceError::OutOfBounds);
            }
            for (word, bytes) in aligned.iter_mut().zip(block.contents.chunks_exact(4)) {
                *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            }
            emmc::write_sector(sector, &aligned).map_err(|e| DeviceError::Emmc(e as u8))?;
            crate::watchdog::feed();
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, DeviceError> {
        Ok(BlockCount(u32::MAX)) // capacity is not read from the card yet
    }
}

/// File timestamps: fixed until the RTC is wired in.
pub struct NoClock;

impl TimeSource for NoClock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 56, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
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
    let manager = VolumeManager::new(Emmc { partition, writable: false }, NoClock);
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
    let manager = VolumeManager::new(Emmc { partition, writable: true }, NoClock);
    let volume = manager.open_volume(VolumeIdx(0)).map_err(|_| 2)?;
    let root = volume.open_root_dir().map_err(|_| 3)?;
    let d = root.open_dir(dir).map_err(|_| 4)?;
    let f = d.open_file_in_dir(file, Mode::ReadWriteCreateOrAppend).map_err(|_| 5)?;
    f.write(data).map_err(|_| 6)?;
    f.close().map_err(|_| 7)?;
    d.close().map_err(|_| 7)?;
    root.close().map_err(|_| 7)?;
    volume.close().map_err(|_| 7)
}

/// Read the last `buf.len()` bytes (or all, if shorter) of `/dir/file`.
/// Returns (file length, bytes read).
pub fn read_tail(partition: Partition, dir: &str, file: &str, buf: &mut [u8]) -> Result<(u32, usize), u8> {
    let manager = VolumeManager::new(Emmc { partition, writable: false }, NoClock);
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
    let manager = VolumeManager::new(Emmc { partition, writable: false }, NoClock);
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
    manager: VolumeManager<Emmc, NoClock>,
    volume: RawVolume,
    file: RawFile,
}

impl FileWriter {
    /// Open `/dirs.../long_name` on `partition` and truncate it. Directory
    /// names are short 8.3 names in upper case; the file is found by its
    /// long name (case-insensitive).
    /// On failure returns the step: 2 volume, 3 root, 4 directory,
    /// 5 file not found, 6 open.
    pub fn rewrite(partition: Partition, dirs: &[&str], long_name: &str) -> Result<Self, u8> {
        let manager = VolumeManager::new(Emmc { partition, writable: true }, NoClock);
        let volume = manager.open_raw_volume(VolumeIdx(0)).map_err(|_| 2)?;
        let mut dir: RawDirectory = manager.open_root_dir(volume).map_err(|_| 3)?;
        for name in dirs {
            let next = manager.open_dir(dir, *name).map_err(|_| 4)?;
            let _ = manager.close_dir(dir);
            dir = next;
        }
        let mut storage = [0u8; 128];
        let mut lfn = LfnBuffer::new(&mut storage);
        let mut short: Option<ShortFileName> = None;
        manager
            .iterate_dir_lfn(dir, &mut lfn, |entry, long| {
                let matches = long.is_some_and(|l| l.eq_ignore_ascii_case(long_name));
                if matches {
                    short = Some(entry.name.clone());
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .map_err(|_| 4)?;
        let short = short.ok_or(5)?;
        let file = manager.open_file_in_dir(dir, &short, Mode::ReadWriteTruncate).map_err(|_| 6)?;
        let _ = manager.close_dir(dir);
        Ok(Self { manager, volume, file })
    }

    /// Append `data` to the file.
    pub fn write(&mut self, data: &[u8]) -> Result<(), ()> {
        self.manager.write(self.file, data).map_err(|_| ())
    }

    /// Close the file, which also stores its final length.
    pub fn finish(self) -> Result<(), ()> {
        let closed = self.manager.close_file(self.file);
        let _ = self.manager.close_volume(self.volume);
        closed.map_err(|_| ())
    }
}
