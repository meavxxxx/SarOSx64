/// Read-only FAT32 filesystem driver.
///
/// Implements the VFS `Filesystem` / `InodeOps` traits so that `ls`, `cat`,
/// `stat`, `cd` etc. work transparently on FAT32 partitions.
use super::vfs::{
    alloc_ino, DirEntry, Errno, FileType, Filesystem, Inode, InodeOps, Stat,
};
use crate::drivers::ide;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

// ─── FAT32 directory entry attribute bits ────────────────────────────────────

const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LFN: u8 = 0x0F; // Long File Name marker

// ─── Shared filesystem context ───────────────────────────────────────────────

struct Fat32Ctx {
    drive: usize,
    part_lba: u64,   // absolute LBA of partition start
    spc: u64,        // sectors per cluster
    fat_start: u64,  // absolute LBA of FAT region
    data_start: u64, // absolute LBA of cluster 2
    root_cluster: u32,
}

impl Fat32Ctx {
    fn cluster_lba(&self, c: u32) -> u64 {
        self.data_start + (c as u64 - 2) * self.spc
    }

    fn cluster_bytes(&self) -> usize {
        self.spc as usize * 512
    }

    fn read_sectors(&self, lba: u64, count: u16, buf: &mut [u8]) -> Result<(), Errno> {
        ide::read_sectors(self.drive, lba, count, buf).map_err(|_| Errno::EIO)
    }

    fn read_cluster(&self, c: u32, buf: &mut [u8]) -> Result<(), Errno> {
        self.read_sectors(self.cluster_lba(c), self.spc as u16, buf)
    }

    fn next_cluster(&self, c: u32) -> Result<Option<u32>, Errno> {
        let byte_off = c as u64 * 4;
        let sec = self.fat_start + byte_off / 512;
        let off = (byte_off % 512) as usize;

        let mut buf = [0u8; 512];
        self.read_sectors(sec, 1, &mut buf)?;

        let entry = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
            & 0x0FFF_FFFF;

        if entry < 2 || entry >= 0x0FFF_FFF8 {
            Ok(None)
        } else {
            Ok(Some(entry))
        }
    }

    fn cluster_chain(&self, start: u32) -> Result<Vec<u32>, Errno> {
        let mut chain = Vec::new();
        let mut c = start;
        while c >= 2 {
            chain.push(c);
            match self.next_cluster(c)? {
                Some(next) => c = next,
                None => break,
            }
            // Safety: limit against infinite loops from corrupted FAT
            if chain.len() > 1_000_000 {
                break;
            }
        }
        Ok(chain)
    }
}

// ─── Directory entry parsing ──────────────────────────────────────────────────

struct FatEntry {
    name: String,
    first_cluster: u32,
    file_size: u32,
    is_dir: bool,
}

fn parse_83_name(raw: &[u8]) -> String {
    let mut name = String::new();
    for i in 0..8 {
        let b = raw[i];
        if b == b' ' {
            break;
        }
        let b = if i == 0 && b == 0x05 { 0xE5 } else { b };
        name.push((b as char).to_ascii_lowercase());
    }
    let mut ext = String::new();
    for i in 8..11 {
        let b = raw[i];
        if b == b' ' {
            break;
        }
        ext.push((b as char).to_ascii_lowercase());
    }
    if ext.is_empty() {
        name
    } else {
        alloc::format!("{}.{}", name, ext)
    }
}

fn lfn_chars(entry: &[u8]) -> [u16; 13] {
    let mut ch = [0u16; 13];
    let pairs: &[(usize, usize)] = &[
        (1, 5),   // bytes 1-10:  chars 0-4
        (14, 6),  // bytes 14-23: chars 5-10
        (28, 2),  // bytes 28-31: chars 11-12
    ];
    let mut idx = 0;
    for &(start, count) in pairs {
        for k in 0..count {
            ch[idx] = u16::from_le_bytes([entry[start + k * 2], entry[start + k * 2 + 1]]);
            idx += 1;
        }
    }
    ch
}

fn read_dir_entries(ctx: &Fat32Ctx, start_cluster: u32) -> Result<Vec<FatEntry>, Errno> {
    let chain = ctx.cluster_chain(start_cluster)?;
    let cs = ctx.cluster_bytes();
    let mut cluster_buf = alloc::vec![0u8; cs];
    let mut entries = Vec::new();
    let mut lfn_chunks: Vec<[u16; 13]> = Vec::new();

    'outer: for &cluster in &chain {
        ctx.read_cluster(cluster, &mut cluster_buf)?;
        let entry_count = cs / 32;

        for e in 0..entry_count {
            let raw = &cluster_buf[e * 32..(e + 1) * 32];
            let first = raw[0];

            if first == 0x00 {
                break 'outer; // end of directory
            }
            if first == 0xE5 {
                lfn_chunks.clear(); // deleted
                continue;
            }

            let attr = raw[11];

            if attr == ATTR_LFN {
                lfn_chunks.push(lfn_chars(raw));
                continue;
            }

            // Skip volume labels
            if attr & ATTR_VOLUME_ID != 0 && attr & ATTR_DIRECTORY == 0 {
                lfn_chunks.clear();
                continue;
            }

            // Skip . and ..
            if raw[0] == b'.' {
                lfn_chunks.clear();
                continue;
            }

            let name = if !lfn_chunks.is_empty() {
                // LFN chunks are in reverse order (last chunk first in the vec)
                let mut s = String::new();
                for chunk in lfn_chunks.iter().rev() {
                    for &c in chunk {
                        if c == 0 || c == 0xFFFF {
                            break;
                        }
                        s.push(char::from_u32(c as u32).unwrap_or('?'));
                    }
                }
                lfn_chunks.clear();
                s
            } else {
                parse_83_name(raw)
            };

            let cluster_hi = u16::from_le_bytes([raw[20], raw[21]]) as u32;
            let cluster_lo = u16::from_le_bytes([raw[26], raw[27]]) as u32;
            let first_cluster = (cluster_hi << 16) | cluster_lo;
            let file_size = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
            let is_dir = attr & ATTR_DIRECTORY != 0;

            entries.push(FatEntry { name, first_cluster, file_size, is_dir });
        }
    }

    Ok(entries)
}

// ─── Directory inode ─────────────────────────────────────────────────────────

struct Fat32DirInode {
    ctx: Arc<Fat32Ctx>,
    cluster: u32,
    ino: u64,
}

impl InodeOps for Fat32DirInode {
    fn stat(&self) -> Stat {
        Stat {
            ino: self.ino,
            kind: FileType::Directory,
            size: 0,
            mode: 0o555,
            nlink: 2,
            uid: 0,
            gid: 0,
        }
    }

    fn lookup(&self, name: &str) -> Result<Arc<Inode>, Errno> {
        let entries = read_dir_entries(&self.ctx, self.cluster)?;
        let name_low = name.to_ascii_lowercase();
        for e in entries {
            if e.name.to_ascii_lowercase() == name_low {
                return Ok(make_inode(&self.ctx, &e));
            }
        }
        Err(Errno::ENOENT)
    }

    fn readdir(&self, offset: usize) -> Result<Option<DirEntry>, Errno> {
        let entries = read_dir_entries(&self.ctx, self.cluster)?;
        Ok(entries.into_iter().nth(offset).map(|e| {
            let kind = if e.is_dir {
                FileType::Directory
            } else {
                FileType::Regular
            };
            DirEntry {
                name: e.name,
                ino: alloc_ino(),
                kind,
            }
        }))
    }

    fn read(&self, _: u64, _: &mut [u8]) -> Result<usize, Errno> {
        Err(Errno::EISDIR)
    }
    fn write(&self, _: u64, _: &[u8]) -> Result<usize, Errno> {
        Err(Errno::ENOTSUP)
    }
    fn truncate(&self, _: u64) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
    fn create(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTSUP)
    }
    fn mkdir(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTSUP)
    }
    fn unlink(&self, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
    fn rmdir(&self, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
    fn symlink(&self, _: &str, _: &str) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTSUP)
    }
    fn readlink(&self) -> Result<String, Errno> {
        Err(Errno::EINVAL)
    }
    fn rename(&self, _: &str, _: &Arc<Inode>, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
    fn insert_child(&self, _: &str, _: Arc<Inode>) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
}

// ─── File inode ───────────────────────────────────────────────────────────────

struct Fat32FileInode {
    ctx: Arc<Fat32Ctx>,
    cluster: u32,
    size: u32,
    ino: u64,
}

impl InodeOps for Fat32FileInode {
    fn stat(&self) -> Stat {
        Stat {
            ino: self.ino,
            kind: FileType::Regular,
            size: self.size as u64,
            mode: 0o444,
            nlink: 1,
            uid: 0,
            gid: 0,
        }
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let size = self.size as u64;
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        let to_read = buf.len().min((size - offset) as usize);
        let ctx = &self.ctx;
        let cs = ctx.cluster_bytes() as u64;
        let chain = ctx.cluster_chain(self.cluster)?;
        let mut cluster_buf = alloc::vec![0u8; cs as usize];
        let mut done = 0usize;

        for (i, &cluster) in chain.iter().enumerate() {
            let cluster_start = i as u64 * cs;
            let cluster_end = cluster_start + cs;
            if cluster_end <= offset {
                continue;
            }
            if cluster_start >= offset + to_read as u64 {
                break;
            }
            ctx.read_cluster(cluster, &mut cluster_buf)?;
            let in_start = if cluster_start < offset {
                (offset - cluster_start) as usize
            } else {
                0
            };
            let in_end = ((offset + to_read as u64).min(cluster_end) - cluster_start) as usize;
            let dst = (cluster_start + in_start as u64 - offset) as usize;
            let len = in_end - in_start;
            buf[dst..dst + len].copy_from_slice(&cluster_buf[in_start..in_end]);
            done += len;
        }
        Ok(done)
    }

    fn write(&self, _: u64, _: &[u8]) -> Result<usize, Errno> {
        Err(Errno::ENOTSUP)
    }
    fn truncate(&self, _: u64) -> Result<(), Errno> {
        Err(Errno::ENOTSUP)
    }
    fn lookup(&self, _: &str) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTDIR)
    }
    fn readdir(&self, _: usize) -> Result<Option<DirEntry>, Errno> {
        Err(Errno::ENOTDIR)
    }
    fn create(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTDIR)
    }
    fn mkdir(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTDIR)
    }
    fn unlink(&self, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTDIR)
    }
    fn rmdir(&self, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTDIR)
    }
    fn symlink(&self, _: &str, _: &str) -> Result<Arc<Inode>, Errno> {
        Err(Errno::ENOTDIR)
    }
    fn readlink(&self) -> Result<String, Errno> {
        Err(Errno::EINVAL)
    }
    fn rename(&self, _: &str, _: &Arc<Inode>, _: &str) -> Result<(), Errno> {
        Err(Errno::ENOTDIR)
    }
    fn insert_child(&self, _: &str, _: Arc<Inode>) -> Result<(), Errno> {
        Err(Errno::ENOTDIR)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_inode(ctx: &Arc<Fat32Ctx>, e: &FatEntry) -> Arc<Inode> {
    let ino = alloc_ino();
    if e.is_dir {
        let ops = Arc::new(Fat32DirInode {
            ctx: Arc::clone(ctx),
            cluster: e.first_cluster,
            ino,
        });
        Inode::new(ino, ops)
    } else {
        let ops = Arc::new(Fat32FileInode {
            ctx: Arc::clone(ctx),
            cluster: e.first_cluster,
            size: e.file_size,
            ino,
        });
        Inode::new(ino, ops)
    }
}

// ─── Filesystem implementation ────────────────────────────────────────────────

struct Fat32Fs {
    ctx: Arc<Fat32Ctx>,
    root: Arc<Inode>,
}

impl Filesystem for Fat32Fs {
    fn root(&self) -> Arc<Inode> {
        Arc::clone(&self.root)
    }
    fn name(&self) -> &'static str {
        "fat32"
    }
}

// ─── Probe / mount ────────────────────────────────────────────────────────────

/// Try to read a FAT32 BPB at `part_lba` on `drive`.
/// Returns a mounted `Filesystem` or None if not FAT32.
pub fn probe(drive: usize, part_lba: u64) -> Option<Arc<dyn Filesystem>> {
    let mut sector = [0u8; 512];
    ide::read_sectors(drive, part_lba, 1, &mut sector).ok()?;

    // Boot sector signature
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }

    let bps = u16::from_le_bytes([sector[11], sector[12]]) as u64;
    if bps != 512 {
        return None; // only 512-byte sectors supported
    }

    let spc              = sector[13] as u64;
    let reserved_sectors = u16::from_le_bytes([sector[14], sector[15]]) as u64;
    let num_fats         = sector[16] as u64;
    let fat_size_16      = u16::from_le_bytes([sector[22], sector[23]]) as u64;
    let fat_size_32      = u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]) as u64;
    let root_cluster     = u32::from_le_bytes([sector[44], sector[45], sector[46], sector[47]]);

    // FAT32 has fat_size_16 == 0 and fat_size_32 > 0
    if fat_size_16 != 0 || fat_size_32 == 0 || spc == 0 {
        return None;
    }

    // Check FS type string ("FAT32   ")
    if &sector[82..87] != b"FAT32" {
        return None;
    }

    let fat_start  = part_lba + reserved_sectors;
    let data_start = fat_start + num_fats * fat_size_32;

    log::info!(
        "FAT32: drive={} part_lba={} spc={} root_cluster={} data_start={}",
        drive, part_lba, spc, root_cluster, data_start
    );

    let ctx = Arc::new(Fat32Ctx {
        drive,
        part_lba,
        spc,
        fat_start,
        data_start,
        root_cluster,
    });

    let root_ino = alloc_ino();
    let root_ops = Arc::new(Fat32DirInode {
        ctx: Arc::clone(&ctx),
        cluster: root_cluster,
        ino: root_ino,
    });
    let root = Inode::new(root_ino, root_ops);

    Some(Arc::new(Fat32Fs { ctx, root }))
}

/// Probe drive for FAT32: try MBR partitions first, then raw sector 0.
pub fn probe_drive(drive: usize) -> Option<Arc<dyn Filesystem>> {
    // Try MBR partition table
    if let Some(parts) = super::mbr::read(drive) {
        for part in parts.iter().flatten() {
            if part.is_fat32() {
                if let Some(fs) = probe(drive, part.lba_start) {
                    return Some(fs);
                }
            }
        }
    }
    // Try raw FAT32 at sector 0
    probe(drive, 0)
}
