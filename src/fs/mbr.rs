/// MBR partition table reader.
/// Reads the first sector of a drive and parses up to 4 primary partition entries.

use crate::drivers::ide;

#[derive(Debug, Clone, Copy)]
pub struct Partition {
    pub status: u8,       // 0x80 = bootable
    pub part_type: u8,    // 0x0B/0x0C = FAT32, 0x83 = Linux ext2/3/4
    pub lba_start: u64,
    pub lba_count: u64,
}

impl Partition {
    pub fn is_fat32(&self) -> bool {
        matches!(self.part_type, 0x0B | 0x0C | 0x1B | 0x1C)
    }
}

/// Read the MBR of `drive` and return up to 4 partition entries.
/// Returns None if no valid MBR signature found.
pub fn read(drive: usize) -> Option<[Option<Partition>; 4]> {
    let mut sector = [0u8; 512];
    ide::read_sectors(drive, 0, 1, &mut sector).ok()?;

    // MBR signature
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }

    let mut parts = [None; 4];
    for i in 0..4 {
        let off = 446 + i * 16;
        let status    = sector[off];
        let part_type = sector[off + 4];
        let lba_start = u32::from_le_bytes([
            sector[off + 8], sector[off + 9], sector[off + 10], sector[off + 11],
        ]) as u64;
        let lba_count = u32::from_le_bytes([
            sector[off + 12], sector[off + 13], sector[off + 14], sector[off + 15],
        ]) as u64;

        if lba_start > 0 && lba_count > 0 {
            parts[i] = Some(Partition { status, part_type, lba_start, lba_count });
        }
    }

    Some(parts)
}
