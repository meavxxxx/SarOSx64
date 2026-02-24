use core::sync::atomic::{AtomicPtr, Ordering};

#[derive(Debug)]
#[repr(C)]
pub struct RequestId(pub u64, pub u64, pub u64, pub u64);

pub const LIMINE_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMapEntryType {
    Usable = 0,
    Reserved = 1,
    AcpiReclaimable = 2,
    AcpiNvs = 3,
    BadMemory = 4,
    BootloaderReclaimable = 5,
    KernelAndModules = 6,
    Framebuffer = 7,
}

#[repr(C)]
#[derive(Debug)]
pub struct MemoryMapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryMapEntryType,
    _pad: u32,
}

#[repr(C)]
pub struct MemoryMapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *const *const MemoryMapEntry,
}

unsafe impl Send for MemoryMapResponse {}
unsafe impl Sync for MemoryMapResponse {}

impl MemoryMapResponse {
    pub fn entries(&self) -> &[*const MemoryMapEntry] {
        unsafe { core::slice::from_raw_parts(self.entries, self.entry_count as usize) }
    }
}

#[repr(C)]
pub struct MemoryMapRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: AtomicPtr<MemoryMapResponse>,
}

unsafe impl Sync for MemoryMapRequest {}

pub static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest {
    id: [
        LIMINE_MAGIC[0],
        LIMINE_MAGIC[1],
        0x67cf3d9d378a806f,
        0xe304acdfc50c3c62,
    ],
    revision: 0,
    response: AtomicPtr::new(core::ptr::null_mut()),
};

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct HhdmRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: AtomicPtr<HhdmResponse>,
}

unsafe impl Sync for HhdmRequest {}

pub static HHDM_REQUEST: HhdmRequest = HhdmRequest {
    id: [
        LIMINE_MAGIC[0],
        LIMINE_MAGIC[1],
        0x48dcf1cb8ad2b852,
        0x63984e959a98244b,
    ],
    revision: 0,
    response: AtomicPtr::new(core::ptr::null_mut()),
};

#[repr(C)]
pub struct KernelAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct KernelAddressRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: AtomicPtr<KernelAddressResponse>,
}

unsafe impl Sync for KernelAddressRequest {}

pub static KERNEL_ADDR_REQUEST: KernelAddressRequest = KernelAddressRequest {
    id: [
        LIMINE_MAGIC[0],
        LIMINE_MAGIC[1],
        0x71ba76863cc55f63,
        0xb2644a48c516a487,
    ],
    revision: 0,
    response: AtomicPtr::new(core::ptr::null_mut()),
};

#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    _pad: [u8; 7],
    pub edid_size: u64,
    pub edid: *const u8,
    pub mode_count: u64,
    pub modes: *const *const u8,
}

unsafe impl Send for Framebuffer {}
unsafe impl Sync for Framebuffer {}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *const *const Framebuffer,
}

unsafe impl Sync for FramebufferResponse {}

impl FramebufferResponse {
    pub fn framebuffers(&self) -> &[*const Framebuffer] {
        unsafe { core::slice::from_raw_parts(self.framebuffers, self.framebuffer_count as usize) }
    }
}

#[repr(C)]
pub struct FramebufferRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: AtomicPtr<FramebufferResponse>,
}

unsafe impl Sync for FramebufferRequest {}

pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest {
    id: [
        LIMINE_MAGIC[0],
        LIMINE_MAGIC[1],
        0x9d5827dcd881dd75,
        0xa3148604f6fab11b,
    ],
    revision: 1,
    response: AtomicPtr::new(core::ptr::null_mut()),
};

#[repr(C)]
pub struct SmpInfo {
    pub processor_id: u32,
    pub lapic_id: u32,
    _reserved: u64,
    pub goto_address: AtomicPtr<unsafe extern "C" fn(*const SmpInfo) -> !>,
    pub extra_arg: u64,
}

#[repr(C)]
pub struct SmpResponse {
    pub revision: u64,
    pub flags: u32,
    pub bsp_lapic: u32,
    pub cpu_count: u64,
    pub cpus: *const *const SmpInfo,
}

unsafe impl Sync for SmpResponse {}

#[repr(C)]
pub struct SmpRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: AtomicPtr<SmpResponse>,
    pub flags: u64,
}

unsafe impl Sync for SmpRequest {}

pub static SMP_REQUEST: SmpRequest = SmpRequest {
    id: [
        LIMINE_MAGIC[0],
        LIMINE_MAGIC[1],
        0x95a67b819a1b857e,
        0xa0b61b723b6a73e0,
    ],
    revision: 0,
    response: AtomicPtr::new(core::ptr::null_mut()),
    flags: 0,
};

pub fn hhdm_offset() -> u64 {
    let resp = HHDM_REQUEST.response.load(Ordering::Relaxed);
    assert!(!resp.is_null(), "Limine HHDM response is null");
    unsafe { (*resp).offset }
}

pub fn phys_to_virt(phys: u64) -> u64 {
    phys + hhdm_offset()
}

pub fn virt_to_phys(virt: u64) -> u64 {
    virt - hhdm_offset()
}
