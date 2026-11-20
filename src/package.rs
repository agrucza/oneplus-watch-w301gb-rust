//! The update package (see `tools/mkota.py`), unpacked while it streams in:
//! a 0xB4-byte header, the app image, and the image's CRC32. The image goes
//! straight into a file through a `FileWriter`.

use crate::storage::FileWriter;

pub const PACKAGE_HEADER: usize = 0xB4;
const PACKAGE_SIZE_FIELD: usize = 0x76;

/// CRC32 as used by zlib (polynomial 0xEDB88320).
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// Running CRC32; start with `0xFFFF_FFFF`, invert at the end.
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = CRC_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

/// Unpacks the update package as it streams in and writes the image file.
pub struct Package {
    writer: Option<FileWriter>,
    header: [u8; PACKAGE_HEADER],
    seen: usize,
    image_len: usize,
    crc: u32,
    trailer: [u8; 4],
    failed: Option<&'static str>,
}

impl Package {
    pub fn new(writer: FileWriter) -> Self {
        Self {
            writer: Some(writer),
            header: [0; PACKAGE_HEADER],
            seen: 0,
            image_len: 0,
            crc: 0xFFFF_FFFF,
            trailer: [0; 4],
            failed: None,
        }
    }

    pub fn feed(&mut self, mut data: &[u8], total: usize) {
        while !data.is_empty() && self.failed.is_none() {
            if self.seen < PACKAGE_HEADER {
                let n = data.len().min(PACKAGE_HEADER - self.seen);
                self.header[self.seen..self.seen + n].copy_from_slice(&data[..n]);
                self.seen += n;
                data = &data[n..];
                if self.seen == PACKAGE_HEADER {
                    let f = &self.header[PACKAGE_SIZE_FIELD..PACKAGE_SIZE_FIELD + 4];
                    self.image_len = u32::from_le_bytes([f[0], f[1], f[2], f[3]]) as usize;
                    if &self.header[..4] != b"~ZZZ" {
                        self.failed = Some("not an update package");
                    } else if PACKAGE_HEADER + self.image_len + 4 != total {
                        self.failed = Some("package size field does not match");
                    }
                }
            } else if self.seen < PACKAGE_HEADER + self.image_len {
                let offset = self.seen - PACKAGE_HEADER;
                let n = data.len().min(self.image_len - offset);
                if offset == 0 && n >= 8 && !plausible_vectors(&data[..8]) {
                    self.failed = Some("image does not start with a valid vector table");
                    return;
                }
                self.crc = crc32_update(self.crc, &data[..n]);
                let ok = self.writer.as_mut().is_some_and(|w| w.write(&data[..n]).is_ok());
                if !ok {
                    self.failed = Some("writing the image file failed");
                    return;
                }
                self.seen += n;
                data = &data[n..];
            } else {
                let offset = self.seen - PACKAGE_HEADER - self.image_len;
                let n = data.len().min(4 - offset);
                self.trailer[offset..offset + n].copy_from_slice(&data[..n]);
                self.seen += n;
                data = &data[n..];
            }
        }
    }

    /// Close the file. `Ok` only if the whole package arrived and its CRC matches.
    pub fn finish(mut self, total: usize) -> Result<usize, &'static str> {
        let closed = self.writer.take().map(|w| w.finish());
        if let Some(reason) = self.failed {
            return Err(reason);
        }
        if self.seen != total {
            return Err("package incomplete");
        }
        if !self.crc != u32::from_le_bytes(self.trailer) {
            return Err("image CRC does not match");
        }
        match closed {
            Some(Ok(())) => Ok(self.image_len),
            _ => Err("closing the image file failed"),
        }
    }
}

/// Initial stack pointer in RAM, reset vector odd and inside the app's flash.
fn plausible_vectors(v: &[u8]) -> bool {
    let sp = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    let reset = u32::from_le_bytes([v[4], v[5], v[6], v[7]]);
    (0x2000_0000 < sp && sp <= 0x200A_0000) && reset & 1 != 0 && (0x0801_E000 < reset && reset < 0x0820_0000)
}
