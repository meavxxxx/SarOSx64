/// ATA PIO driver for IDE controllers.
/// Supports LBA28/LBA48, master/slave on primary and secondary channels.
use crate::arch::x86_64::io::{inb, inw, outb, outw};
use crate::sync::spinlock::SpinLock;
use alloc::string::String;
use alloc::vec::Vec;

// ─── Channel I/O base addresses ───────────────────────────────────────────────

const PRIMARY_BASE: u16   = 0x1F0;
const PRIMARY_CTRL: u16   = 0x3F6;
const SECONDARY_BASE: u16 = 0x170;
const SECONDARY_CTRL: u16 = 0x376;

// ─── Register offsets from base ───────────────────────────────────────────────

const REG_DATA:     u16 = 0x00; // 16-bit
const REG_ERROR:    u16 = 0x01;
const REG_FEATURES: u16 = 0x01;
const REG_SECCOUNT: u16 = 0x02;
const REG_LBA0:     u16 = 0x03;
const REG_LBA1:     u16 = 0x04;
const REG_LBA2:     u16 = 0x05;
const REG_HDDEVSEL: u16 = 0x06;
const REG_STATUS:   u16 = 0x07;
const REG_COMMAND:  u16 = 0x07;

// ─── Status bits ──────────────────────────────────────────────────────────────

const SR_BSY:  u8 = 0x80;
const SR_DRDY: u8 = 0x40;
const SR_DRQ:  u8 = 0x08;
const SR_ERR:  u8 = 0x01;
const SR_DF:   u8 = 0x20;

// ─── ATA commands ─────────────────────────────────────────────────────────────

const CMD_READ_PIO:    u8 = 0x20;
const CMD_READ_PIO_EX: u8 = 0x24; // LBA48
const CMD_WRITE_PIO:   u8 = 0x30;
const CMD_WRITE_PIO_EX:u8 = 0x34; // LBA48
const CMD_CACHE_FLUSH: u8 = 0xE7;
const CMD_IDENTIFY:    u8 = 0xEC;

pub const SECTOR_SIZE: usize = 512;

// ─── Drive ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Drive {
    pub channel: u8,   // 0 = primary, 1 = secondary
    pub drive: u8,     // 0 = master, 1 = slave
    pub lba48: bool,
    pub sectors: u64,
    pub model: String,
    pub serial: String,
    base: u16,
    ctrl: u16,
}

impl Drive {
    pub fn size_mb(&self) -> u64 {
        self.sectors * SECTOR_SIZE as u64 / 1024 / 1024
    }
}

// ─── Low-level helpers ────────────────────────────────────────────────────────

fn status(base: u16) -> u8 {
    unsafe { inb(base + REG_STATUS) }
}

/// 400ns delay: read Alt Status 4 times
fn delay400(ctrl: u16) {
    for _ in 0..4 {
        unsafe { inb(ctrl); }
    }
}

/// Wait until BSY clears; returns false on timeout or error
fn wait_bsy(base: u16) -> bool {
    for _ in 0..100_000u32 {
        let s = status(base);
        if s & SR_BSY == 0 {
            return true;
        }
    }
    false
}

/// Wait until DRQ or ERR
fn wait_drq(base: u16) -> Result<(), &'static str> {
    for _ in 0..100_000u32 {
        let s = status(base);
        if s & SR_ERR != 0 || s & SR_DF != 0 {
            return Err("ATA error/device-fault");
        }
        if s & SR_DRQ != 0 {
            return Ok(());
        }
    }
    Err("ATA DRQ timeout")
}

fn select_drive(base: u16, ctrl: u16, drive: u8, lba_top: u8) {
    unsafe {
        outb(base + REG_HDDEVSEL, 0xE0 | ((drive & 1) << 4) | (lba_top & 0x0F));
    }
    delay400(ctrl);
}

fn ata_string(words: &[u16], word_start: usize, word_count: usize) -> String {
    let mut bytes = Vec::with_capacity(word_count * 2);
    for w in &words[word_start..word_start + word_count] {
        bytes.push((w >> 8) as u8);
        bytes.push((w & 0xFF) as u8);
    }
    // trim trailing spaces
    let s = core::str::from_utf8(&bytes).unwrap_or("").trim_end();
    String::from(s)
}

// ─── Identify ────────────────────────────────────────────────────────────────

fn identify(base: u16, ctrl: u16, drive_sel: u8) -> Option<[u16; 256]> {
    unsafe {
        // Select drive, no LBA bits needed for IDENTIFY
        outb(base + REG_HDDEVSEL, 0xA0 | ((drive_sel & 1) << 4));
        delay400(ctrl);

        // Zero sector count/LBA registers
        outb(base + REG_SECCOUNT, 0);
        outb(base + REG_LBA0, 0);
        outb(base + REG_LBA1, 0);
        outb(base + REG_LBA2, 0);

        outb(base + REG_COMMAND, CMD_IDENTIFY);
        delay400(ctrl);

        let s = inb(base + REG_STATUS);
        if s == 0 {
            return None; // no drive
        }

        if !wait_bsy(base) {
            return None;
        }

        // Check if ATAPI (LBA1/LBA2 non-zero = not plain ATA)
        let lba1 = inb(base + REG_LBA1);
        let lba2 = inb(base + REG_LBA2);
        if lba1 != 0 || lba2 != 0 {
            return None; // ATAPI — skip for now
        }

        if wait_drq(base).is_err() {
            return None;
        }

        let mut buf = [0u16; 256];
        for w in buf.iter_mut() {
            *w = inw(base + REG_DATA);
        }
        Some(buf)
    }
}

// ─── Public read/write ───────────────────────────────────────────────────────

static DRIVES: SpinLock<Vec<Drive>> = SpinLock::new(Vec::new());

fn with_drive<F, R>(idx: usize, f: F) -> Option<R>
where
    F: FnOnce(&Drive) -> R,
{
    DRIVES.lock().get(idx).map(|d| f(d))
}

/// Read `count` sectors starting at `lba` into `buf`.
/// `buf` must be exactly `count * 512` bytes.
pub fn read_sectors(idx: usize, lba: u64, count: u16, buf: &mut [u8]) -> Result<(), &'static str> {
    assert_eq!(buf.len(), count as usize * SECTOR_SIZE);

    let (base, ctrl, drive_sel, lba48) = {
        let drives = DRIVES.lock();
        let d = drives.get(idx).ok_or("no such drive")?;
        (d.base, d.ctrl, d.drive, d.lba48)
    };

    if lba48 {
        read_lba48(base, ctrl, drive_sel, lba, count, buf)
    } else {
        read_lba28(base, ctrl, drive_sel, lba as u32, count as u8, buf)
    }
}

/// Write `count` sectors starting at `lba` from `buf`.
pub fn write_sectors(idx: usize, lba: u64, count: u16, buf: &[u8]) -> Result<(), &'static str> {
    assert_eq!(buf.len(), count as usize * SECTOR_SIZE);

    let (base, ctrl, drive_sel, lba48) = {
        let drives = DRIVES.lock();
        let d = drives.get(idx).ok_or("no such drive")?;
        (d.base, d.ctrl, d.drive, d.lba48)
    };

    if lba48 {
        write_lba48(base, ctrl, drive_sel, lba, count, buf)
    } else {
        write_lba28(base, ctrl, drive_sel, lba as u32, count as u8, buf)
    }
}

// ─── LBA28 ───────────────────────────────────────────────────────────────────

fn read_lba28(base: u16, ctrl: u16, drive: u8, lba: u32, count: u8, buf: &mut [u8]) -> Result<(), &'static str> {
    select_drive(base, ctrl, drive, ((lba >> 24) & 0x0F) as u8);
    if !wait_bsy(base) { return Err("BSY timeout"); }

    unsafe {
        outb(base + REG_FEATURES, 0);
        outb(base + REG_SECCOUNT, count);
        outb(base + REG_LBA0, (lba & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 8) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 16) & 0xFF) as u8);
        outb(base + REG_COMMAND, CMD_READ_PIO);
    }

    for sec in 0..count as usize {
        delay400(ctrl);
        if !wait_bsy(base) { return Err("BSY timeout"); }
        wait_drq(base)?;

        let off = sec * SECTOR_SIZE;
        unsafe {
            for i in (0..SECTOR_SIZE).step_by(2) {
                let w = inw(base + REG_DATA);
                buf[off + i]     = (w & 0xFF) as u8;
                buf[off + i + 1] = (w >> 8) as u8;
            }
        }
    }
    Ok(())
}

fn write_lba28(base: u16, ctrl: u16, drive: u8, lba: u32, count: u8, buf: &[u8]) -> Result<(), &'static str> {
    select_drive(base, ctrl, drive, ((lba >> 24) & 0x0F) as u8);
    if !wait_bsy(base) { return Err("BSY timeout"); }

    unsafe {
        outb(base + REG_FEATURES, 0);
        outb(base + REG_SECCOUNT, count);
        outb(base + REG_LBA0, (lba & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 8) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 16) & 0xFF) as u8);
        outb(base + REG_COMMAND, CMD_WRITE_PIO);
    }

    for sec in 0..count as usize {
        delay400(ctrl);
        if !wait_bsy(base) { return Err("BSY timeout"); }
        wait_drq(base)?;

        let off = sec * SECTOR_SIZE;
        unsafe {
            for i in (0..SECTOR_SIZE).step_by(2) {
                let w = (buf[off + i] as u16) | ((buf[off + i + 1] as u16) << 8);
                outw(base + REG_DATA, w);
            }
        }
    }

    unsafe { outb(base + REG_COMMAND, CMD_CACHE_FLUSH); }
    if !wait_bsy(base) { return Err("flush timeout"); }
    Ok(())
}

// ─── LBA48 ───────────────────────────────────────────────────────────────────

fn read_lba48(base: u16, ctrl: u16, drive: u8, lba: u64, count: u16, buf: &mut [u8]) -> Result<(), &'static str> {
    select_drive(base, ctrl, drive, 0);
    if !wait_bsy(base) { return Err("BSY timeout"); }

    unsafe {
        outb(base + REG_FEATURES, 0);
        outb(base + REG_FEATURES, 0);
        outb(base + REG_SECCOUNT, (count >> 8) as u8);
        outb(base + REG_LBA0, ((lba >> 24) & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 32) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 40) & 0xFF) as u8);
        outb(base + REG_SECCOUNT, (count & 0xFF) as u8);
        outb(base + REG_LBA0, (lba & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 8) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 16) & 0xFF) as u8);
        outb(base + REG_COMMAND, CMD_READ_PIO_EX);
    }

    for sec in 0..count as usize {
        delay400(ctrl);
        if !wait_bsy(base) { return Err("BSY timeout"); }
        wait_drq(base)?;

        let off = sec * SECTOR_SIZE;
        unsafe {
            for i in (0..SECTOR_SIZE).step_by(2) {
                let w = inw(base + REG_DATA);
                buf[off + i]     = (w & 0xFF) as u8;
                buf[off + i + 1] = (w >> 8) as u8;
            }
        }
    }
    Ok(())
}

fn write_lba48(base: u16, ctrl: u16, drive: u8, lba: u64, count: u16, buf: &[u8]) -> Result<(), &'static str> {
    select_drive(base, ctrl, drive, 0);
    if !wait_bsy(base) { return Err("BSY timeout"); }

    unsafe {
        outb(base + REG_FEATURES, 0);
        outb(base + REG_FEATURES, 0);
        outb(base + REG_SECCOUNT, (count >> 8) as u8);
        outb(base + REG_LBA0, ((lba >> 24) & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 32) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 40) & 0xFF) as u8);
        outb(base + REG_SECCOUNT, (count & 0xFF) as u8);
        outb(base + REG_LBA0, (lba & 0xFF) as u8);
        outb(base + REG_LBA1, ((lba >> 8) & 0xFF) as u8);
        outb(base + REG_LBA2, ((lba >> 16) & 0xFF) as u8);
        outb(base + REG_COMMAND, CMD_WRITE_PIO_EX);
    }

    for sec in 0..count as usize {
        delay400(ctrl);
        if !wait_bsy(base) { return Err("BSY timeout"); }
        wait_drq(base)?;

        let off = sec * SECTOR_SIZE;
        unsafe {
            for i in (0..SECTOR_SIZE).step_by(2) {
                let w = (buf[off + i] as u16) | ((buf[off + i + 1] as u16) << 8);
                outw(base + REG_DATA, w);
            }
        }
    }

    unsafe { outb(base + REG_COMMAND, CMD_CACHE_FLUSH); }
    if !wait_bsy(base) { return Err("flush timeout"); }
    Ok(())
}

// ─── Init ─────────────────────────────────────────────────────────────────────

fn probe_channel(channel: u8, base: u16, ctrl: u16, list: &mut Vec<Drive>) {
    for drive_sel in 0u8..2 {
        let Some(id) = identify(base, ctrl, drive_sel) else { continue };

        // word 83 bit 10 = LBA48 support
        let lba48 = id[83] & (1 << 10) != 0;

        let sectors = if lba48 {
            (id[100] as u64)
                | ((id[101] as u64) << 16)
                | ((id[102] as u64) << 32)
                | ((id[103] as u64) << 48)
        } else {
            (id[60] as u64) | ((id[61] as u64) << 16)
        };

        if sectors == 0 {
            continue;
        }

        let model  = ata_string(&id, 27, 20);
        let serial = ata_string(&id, 10, 10);

        list.push(Drive {
            channel,
            drive: drive_sel,
            lba48,
            sectors,
            model,
            serial,
            base,
            ctrl,
        });
    }
}

pub fn init() {
    let mut list = Vec::new();

    probe_channel(0, PRIMARY_BASE,   PRIMARY_CTRL,   &mut list);
    probe_channel(1, SECONDARY_BASE, SECONDARY_CTRL, &mut list);

    if list.is_empty() {
        log::info!("IDE: no drives found");
    } else {
        for (i, d) in list.iter().enumerate() {
            log::info!(
                "IDE: drive {} — {} [{} MiB, LBA{}]  s/n: {}",
                i, d.model, d.size_mb(),
                if d.lba48 { 48 } else { 28 },
                d.serial,
            );
        }
    }

    *DRIVES.lock() = list;
}

/// Number of detected drives
pub fn drive_count() -> usize {
    DRIVES.lock().len()
}

pub fn drive_info(idx: usize) -> Option<Drive> {
    DRIVES.lock().get(idx).cloned()
}
