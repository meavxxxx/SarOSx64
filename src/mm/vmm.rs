use crate::arch::x86_64::io::invlpg;
use crate::arch::x86_64::limine::{phys_to_virt, virt_to_phys};
use crate::mm::pmm::{align_down, align_up, alloc_zeroed_frame, free_frame, PAGE_SIZE};
use crate::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
pub const PTE_PWT: u64 = 1 << 3;
pub const PTE_PCD: u64 = 1 << 4;
pub const PTE_ACCESSED: u64 = 1 << 5;
pub const PTE_DIRTY: u64 = 1 << 6;
pub const PTE_LARGE: u64 = 1 << 7;
pub const PTE_GLOBAL: u64 = 1 << 8;
pub const PTE_NO_EXEC: u64 = 1 << 63;
pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

impl PageTable {
    pub fn zero(&mut self) {
        self.entries.fill(0);
    }

    pub fn get_entry(&self, idx: usize) -> u64 {
        self.entries[idx]
    }

    pub fn set_entry(&mut self, idx: usize, val: u64) {
        self.entries[idx] = val;
    }

    pub fn is_present(&self, idx: usize) -> bool {
        self.entries[idx] & PTE_PRESENT != 0
    }

    pub fn get_or_alloc_table(&mut self, idx: usize, flags: u64) -> Option<&'static mut PageTable> {
        if !self.is_present(idx) {
            let phys = alloc_zeroed_frame()?;
            self.entries[idx] = phys | flags | PTE_PRESENT;
        }
        let phys = self.entries[idx] & PTE_ADDR_MASK;
        Some(unsafe { &mut *(phys_to_virt(phys) as *mut PageTable) })
    }

    pub fn get_table(&self, idx: usize) -> Option<&'static mut PageTable> {
        if !self.is_present(idx) {
            return None;
        }
        let phys = self.entries[idx] & PTE_ADDR_MASK;
        Some(unsafe { &mut *(phys_to_virt(phys) as *mut PageTable) })
    }
}

#[inline]
fn pml4_idx(vaddr: u64) -> usize {
    ((vaddr >> 39) & 0x1FF) as usize
}
#[inline]
fn pdpt_idx(vaddr: u64) -> usize {
    ((vaddr >> 30) & 0x1FF) as usize
}
#[inline]
fn pd_idx(vaddr: u64) -> usize {
    ((vaddr >> 21) & 0x1FF) as usize
}
#[inline]
fn pt_idx(vaddr: u64) -> usize {
    ((vaddr >> 12) & 0x1FF) as usize
}

pub struct AddressSpace {
    pub pml4_phys: u64,
}

impl AddressSpace {
    pub fn new_user() -> Option<Self> {
        let pml4_phys = alloc_zeroed_frame()?;
        let mut space = Self { pml4_phys };

        let kernel_pml4 = get_kernel_pml4();
        let user_pml4 = space.pml4_mut();

        for i in 256..512 {
            user_pml4.entries[i] = kernel_pml4.entries[i];
        }

        Some(space)
    }

    pub fn kernel() -> Self {
        let cr3: u64;
        unsafe { core::arch::asm!("mov %cr3, {}", out(reg) cr3, options(att_syntax)) };
        Self {
            pml4_phys: cr3 & PTE_ADDR_MASK,
        }
    }

    fn pml4_mut(&mut self) -> &mut PageTable {
        unsafe { &mut *(phys_to_virt(self.pml4_phys) as *mut PageTable) }
    }

    fn pml4(&self) -> &PageTable {
        unsafe { &*(phys_to_virt(self.pml4_phys) as *const PageTable) }
    }

    pub fn map(&mut self, virt: u64, phys: u64, flags: u64) -> bool {
        let pml4 = self.pml4_mut();

        let parent_flags = PTE_WRITABLE | PTE_USER;

        let pdpt = pml4.get_or_alloc_table(pml4_idx(virt), parent_flags);
        let pdpt = match pdpt {
            Some(t) => t,
            None => return false,
        };

        let pd = pdpt.get_or_alloc_table(pdpt_idx(virt), parent_flags);
        let pd = match pd {
            Some(t) => t,
            None => return false,
        };

        let pt = pd.get_or_alloc_table(pd_idx(virt), parent_flags);
        let pt = match pt {
            Some(t) => t,
            None => return false,
        };

        pt.set_entry(pt_idx(virt), (phys & PTE_ADDR_MASK) | flags | PTE_PRESENT);

        unsafe {
            invlpg(virt);
        }
        true
    }

    pub fn map_large(&mut self, virt: u64, phys: u64, flags: u64) -> bool {
        assert!(virt % (2 * 1024 * 1024) == 0, "virt must be 2MiB aligned");
        assert!(phys % (2 * 1024 * 1024) == 0, "phys must be 2MiB aligned");

        let pml4 = self.pml4_mut();
        let parent_flags = PTE_WRITABLE | PTE_USER;

        let pdpt = pml4.get_or_alloc_table(pml4_idx(virt), parent_flags)?;
        let pd = pdpt.get_or_alloc_table(pdpt_idx(virt), parent_flags)?;

        pd.set_entry(
            pd_idx(virt),
            (phys & PTE_ADDR_MASK) | flags | PTE_PRESENT | PTE_LARGE,
        );
        unsafe {
            invlpg(virt);
        }
        true
    }

    pub fn map_range(&mut self, virt: u64, phys: u64, size: u64, flags: u64) -> bool {
        let mut offset = 0u64;
        while offset < size {
            let v = virt + offset;
            let p = phys + offset;

            if size - offset >= 2 * 1024 * 1024
                && v % (2 * 1024 * 1024) == 0
                && p % (2 * 1024 * 1024) == 0
            {
                if !self.map_large(v, p, flags) {
                    return false;
                }
                offset += 2 * 1024 * 1024;
            } else {
                if !self.map(v, p, flags) {
                    return false;
                }
                offset += PAGE_SIZE as u64;
            }
        }
        true
    }

    pub fn unmap(&mut self, virt: u64) {
        let pml4 = self.pml4_mut();

        if let Some(pdpt) = pml4.get_table(pml4_idx(virt)) {
            if let Some(pd) = pdpt.get_table(pdpt_idx(virt)) {
                if let Some(pt) = pd.get_table(pd_idx(virt)) {
                    pt.set_entry(pt_idx(virt), 0);
                    unsafe {
                        invlpg(virt);
                    }
                }
            }
        }
    }

    pub fn translate(&self, virt: u64) -> Option<u64> {
        let pml4 = self.pml4();
        let pdpt = pml4.get_table(pml4_idx(virt))?;
        let pd = pdpt.get_table(pdpt_idx(virt))?;

        let pd_entry = pd.get_entry(pd_idx(virt));
        if pd_entry & PTE_LARGE != 0 {
            let base = pd_entry & PTE_ADDR_MASK;
            return Some(base + (virt & 0x1F_FFFF));
        }

        let pt = pd.get_table(pd_idx(virt))?;
        let entry = pt.get_entry(pt_idx(virt));
        if entry & PTE_PRESENT == 0 {
            return None;
        }

        let phys = (entry & PTE_ADDR_MASK) | (virt & 0xFFF);
        Some(phys)
    }

    pub fn activate(&self) {
        unsafe {
            core::arch::asm!(
                "mov {}, %cr3",
                in(reg) self.pml4_phys,
                options(att_syntax, nostack)
            );
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        free_user_page_tables(self.pml4_phys);
    }
}

fn free_user_page_tables(pml4_phys: u64) {
    let pml4 = unsafe { &*(phys_to_virt(pml4_phys) as *const PageTable) };

    for i in 0..256usize {
        if !pml4.is_present(i) {
            continue;
        }
        let pdpt_phys = pml4.entries[i] & PTE_ADDR_MASK;
        let pdpt = unsafe { &*(phys_to_virt(pdpt_phys) as *const PageTable) };

        for j in 0..512usize {
            if !pdpt.is_present(j) {
                continue;
            }
            let pd_phys = pdpt.entries[j] & PTE_ADDR_MASK;
            let pd = unsafe { &*(phys_to_virt(pd_phys) as *const PageTable) };

            for k in 0..512usize {
                if !pd.is_present(k) {
                    continue;
                }
                if pd.entries[k] & PTE_LARGE != 0 {
                    continue;
                } // Large Page
                let pt_phys = pd.entries[k] & PTE_ADDR_MASK;
                free_frame(pt_phys);
            }
            free_frame(pd_phys);
        }
        free_frame(pdpt_phys);
    }
    free_frame(pml4_phys);
}

static mut KERNEL_PML4_PHYS: u64 = 0;

fn get_kernel_pml4() -> &'static mut PageTable {
    unsafe {
        assert!(KERNEL_PML4_PHYS != 0, "Kernel PML4 not initialized");
        &mut *(phys_to_virt(KERNEL_PML4_PHYS) as *mut PageTable)
    }
}

pub fn init() {
    let cr3: u64;
    unsafe { core::arch::asm!("mov %cr3, {}", out(reg) cr3, options(att_syntax)) };
    unsafe {
        KERNEL_PML4_PHYS = cr3 & PTE_ADDR_MASK;
    }

    log::info!("VMM: kernel PML4 at phys={:#012x}", unsafe {
        KERNEL_PML4_PHYS
    });
}

pub fn handle_page_fault(addr: u64, error: u64) -> bool {
    let present = error & 1 != 0;
    let write = error & 2 != 0;
    let user = error & 4 != 0;

    let proc = match crate::proc::scheduler::current_process() {
        Some(p) => p,
        None => return false,
    };

    let mut proc = proc.lock();

    let vma = proc.vm.find_vma(addr);
    let vma = match vma {
        Some(v) => v,
        None => return false,
    };

    if write && !vma.flags.contains(VmaFlags::WRITE) {
        if vma.flags.contains(VmaFlags::COPY_ON_WRITE) {
            return handle_cow(&mut proc.address_space, addr);
        }
        return false;
    }

    if !present {
        return handle_demand_page(&mut proc.address_space, addr, vma);
    }

    false
}

fn handle_demand_page(space: &mut AddressSpace, addr: u64, vma: &VmaEntry) -> bool {
    let page_addr = align_down(addr, PAGE_SIZE as u64);
    let phys = match alloc_zeroed_frame() {
        Some(p) => p,
        None => return false,
    };

    let mut flags = PTE_PRESENT | PTE_USER;
    if vma.flags.contains(VmaFlags::WRITE) {
        flags |= PTE_WRITABLE;
    }
    if !vma.flags.contains(VmaFlags::EXEC) {
        flags |= PTE_NO_EXEC;
    }

    space.map(page_addr, phys, flags)
}

fn handle_cow(space: &mut AddressSpace, addr: u64) -> bool {
    let page_addr = align_down(addr, PAGE_SIZE as u64);

    let old_phys = match space.translate(page_addr) {
        Some(p) => p,
        None => return false,
    };

    let new_phys = match alloc_zeroed_frame() {
        Some(p) => p,
        None => return false,
    };

    unsafe {
        let src = phys_to_virt(old_phys) as *const u8;
        let dst = phys_to_virt(new_phys) as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
    }

    space.map(page_addr, new_phys, PTE_PRESENT | PTE_WRITABLE | PTE_USER);
    true
}

bitflags::bitflags! {
    pub struct VmaFlags: u32 {
        const READ          = 1 << 0;
        const WRITE         = 1 << 1;
        const EXEC          = 1 << 2;
        const SHARED        = 1 << 3;
        const COPY_ON_WRITE = 1 << 4;
        const ANONYMOUS     = 1 << 5;
        const GROWS_DOWN    = 1 << 6;
    }
}

#[derive(Debug, Clone)]
pub struct VmaEntry {
    pub start: u64,
    pub end: u64,
    pub flags: VmaFlags,
}

impl VmaEntry {
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end
    }
}

pub struct VmSpace {
    areas: alloc::vec::Vec<VmaEntry>,
    pub brk: u64,
}

impl VmSpace {
    pub fn new() -> Self {
        Self {
            areas: alloc::vec::Vec::new(),
            brk: 0x1000_0000,
        }
    }

    pub fn find_vma(&self, addr: u64) -> Option<&VmaEntry> {
        self.areas.iter().find(|a| a.contains(addr))
    }

    pub fn add_vma(&mut self, start: u64, end: u64, flags: VmaFlags) {
        self.areas.push(VmaEntry { start, end, flags });
        self.areas.sort_unstable_by_key(|a| a.start);
    }

    pub fn remove_vma(&mut self, start: u64, end: u64) {
        self.areas.retain(|a| !(a.start >= start && a.end <= end));
    }
}
