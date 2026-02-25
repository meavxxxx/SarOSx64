use crate::arch::x86_64::io::{inl, outl};
use crate::sync::spinlock::SpinLock;
use alloc::vec::Vec;

// ─── Config-space I/O ────────────────────────────────────────────────────────

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn make_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    (1u32 << 31)
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

pub fn read_u32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, make_addr(bus, dev, func, offset));
        inl(CONFIG_DATA)
    }
}

pub fn write_u32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, make_addr(bus, dev, func, offset));
        outl(CONFIG_DATA, val);
    }
}

pub fn read_u16(bus: u8, dev: u8, func: u8, offset: u8) -> u16 {
    let dword = read_u32(bus, dev, func, offset & !3);
    (dword >> ((offset & 2) * 8)) as u16
}

pub fn read_u8(bus: u8, dev: u8, func: u8, offset: u8) -> u8 {
    let dword = read_u32(bus, dev, func, offset & !3);
    (dword >> ((offset & 3) * 8)) as u8
}

// Enable Bus Master + Memory Space + I/O Space in command register
pub fn enable_bus_master(bus: u8, dev: u8, func: u8) {
    let cmd = read_u16(bus, dev, func, 0x04);
    write_u32(bus, dev, func, 0x04, (cmd | 0x0007) as u32);
}

// ─── PCI device descriptor ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,

    pub vendor_id: u16,
    pub device_id: u16,

    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,

    pub header_type: u8,

    /// BAR0–BAR5 raw values (type-0 header only; bridges have 2)
    pub bars: [u32; 6],
    pub irq_line: u8,
    pub irq_pin: u8,
}

impl PciDevice {
    pub fn class_name(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x00, 0x01) => "VGA-Compatible (old)",
            (0x01, 0x00) => "SCSI Bus Controller",
            (0x01, 0x01) => "IDE Controller",
            (0x01, 0x05) => "ATA Controller",
            (0x01, 0x06) => "SATA/AHCI Controller",
            (0x01, 0x08) => "NVMe Controller",
            (0x01, _)    => "Mass Storage Controller",
            (0x02, 0x00) => "Ethernet Controller",
            (0x02, 0x80) => "Network Controller",
            (0x02, _)    => "Network Controller",
            (0x03, 0x00) => "VGA Compatible Controller",
            (0x03, 0x01) => "XGA Controller",
            (0x03, _)    => "Display Controller",
            (0x04, _)    => "Multimedia Controller",
            (0x05, _)    => "Memory Controller",
            (0x06, 0x00) => "Host Bridge",
            (0x06, 0x01) => "ISA Bridge",
            (0x06, 0x04) => "PCI-to-PCI Bridge",
            (0x06, _)    => "Bridge Device",
            (0x07, _)    => "Communication Controller",
            (0x08, 0x00) => "PIC",
            (0x08, 0x01) => "DMA Controller",
            (0x08, 0x02) => "Timer",
            (0x08, _)    => "Generic System Peripheral",
            (0x09, _)    => "Input Device",
            (0x0A, _)    => "Docking Station",
            (0x0B, _)    => "Processor",
            (0x0C, 0x00) => "FireWire Controller",
            (0x0C, 0x03) => "USB Controller",
            (0x0C, 0x05) => "SMBus",
            (0x0C, _)    => "Serial Bus Controller",
            (0x0D, _)    => "Wireless Controller",
            (0x0F, _)    => "Satellite Communication",
            (0x10, _)    => "Encryption Controller",
            (0x11, _)    => "Signal Processing Controller",
            _            => "Unknown Device",
        }
    }

    /// Returns true if this device is an AHCI SATA controller
    pub fn is_ahci(&self) -> bool {
        self.class == 0x01 && self.subclass == 0x06 && self.prog_if == 0x01
    }

    /// Returns true if this device looks like an IDE controller
    pub fn is_ide(&self) -> bool {
        self.class == 0x01 && self.subclass == 0x01
    }

    /// BAR base address (strips type bits)
    pub fn bar_base(&self, n: usize) -> u64 {
        let raw = self.bars[n] as u64;
        if raw & 1 == 0 {
            // Memory BAR
            match (raw >> 1) & 0x3 {
                2 => {
                    // 64-bit: next BAR is upper 32 bits
                    if n + 1 < 6 {
                        ((self.bars[n + 1] as u64) << 32) | (raw & !0xF)
                    } else {
                        raw & !0xF
                    }
                }
                _ => raw & !0xF,
            }
        } else {
            // I/O BAR
            raw & !0x3
        }
    }

    pub fn bar_is_io(&self, n: usize) -> bool {
        self.bars[n] & 1 != 0
    }
}

// ─── Global device list ───────────────────────────────────────────────────────

static DEVICES: SpinLock<Vec<PciDevice>> = SpinLock::new(Vec::new());

pub fn devices<F: FnMut(&PciDevice)>(mut f: F) {
    for d in DEVICES.lock().iter() {
        f(d);
    }
}

pub fn find<F: Fn(&PciDevice) -> bool>(pred: F) -> Option<PciDevice> {
    DEVICES.lock().iter().find(|d| pred(d)).cloned()
}

// ─── Scanner ─────────────────────────────────────────────────────────────────

fn probe(bus: u8, dev: u8, func: u8, list: &mut Vec<PciDevice>) {
    let id = read_u32(bus, dev, func, 0x00);
    let vendor_id = (id & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return;
    }
    let device_id = (id >> 16) as u16;

    let class_dword = read_u32(bus, dev, func, 0x08);
    let revision  = (class_dword & 0xFF) as u8;
    let prog_if   = ((class_dword >> 8) & 0xFF) as u8;
    let subclass  = ((class_dword >> 16) & 0xFF) as u8;
    let class     = ((class_dword >> 24) & 0xFF) as u8;

    let header_type = read_u8(bus, dev, func, 0x0E) & 0x7F;

    let mut bars = [0u32; 6];
    if header_type == 0x00 {
        for i in 0..6usize {
            bars[i] = read_u32(bus, dev, func, 0x10 + (i as u8) * 4);
        }
    } else if header_type == 0x01 {
        bars[0] = read_u32(bus, dev, func, 0x10);
        bars[1] = read_u32(bus, dev, func, 0x14);
    }

    let irq_dword = read_u32(bus, dev, func, 0x3C);
    let irq_line = (irq_dword & 0xFF) as u8;
    let irq_pin  = ((irq_dword >> 8) & 0xFF) as u8;

    list.push(PciDevice {
        bus, dev, func,
        vendor_id, device_id,
        class, subclass, prog_if, revision,
        header_type, bars, irq_line, irq_pin,
    });
}

fn scan_bus(bus: u8, list: &mut Vec<PciDevice>) {
    for dev in 0u8..32 {
        let id = read_u32(bus, dev, 0, 0x00);
        if (id & 0xFFFF) as u16 == 0xFFFF {
            continue;
        }

        let htype = read_u8(bus, dev, 0, 0x0E);
        let multi = htype & 0x80 != 0;

        probe(bus, dev, 0, list);

        if multi {
            for func in 1u8..8 {
                probe(bus, dev, func, list);
            }
        }
    }
}

pub fn init() {
    let mut list = Vec::new();

    // Check if the host controller itself is multi-function
    // (indicates multiple PCI host bridges / bus segments)
    let htype = read_u8(0, 0, 0, 0x0E);
    if htype & 0x80 == 0 {
        scan_bus(0, &mut list);
    } else {
        for func in 0u8..8 {
            if (read_u32(0, 0, func, 0x00) & 0xFFFF) as u16 == 0xFFFF {
                break;
            }
            scan_bus(func, &mut list);
        }
    }

    log::info!("PCI: found {} device(s)", list.len());
    for d in &list {
        log::info!(
            "  {:02x}:{:02x}.{} [{:04x}:{:04x}] {} (class {:02x}:{:02x})",
            d.bus, d.dev, d.func,
            d.vendor_id, d.device_id,
            d.class_name(),
            d.class, d.subclass,
        );
    }

    *DEVICES.lock() = list;
}
