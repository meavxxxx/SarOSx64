use crate::arch::x86_64::limine::{phys_to_virt, MemoryMapEntryType, MEMMAP_REQUEST};
use crate::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

pub const MAX_ORDER: usize = 12;

static FREE_PAGES: AtomicUsize = AtomicUsize::new(0);
static TOTAL_PAGES: AtomicUsize = AtomicUsize::new(0);

struct FreeBlock {
    next: *mut FreeBlock,
}

struct FreeList {
    head: *mut FreeBlock,
    count: usize,
}

unsafe impl Send for FreeList {}

impl FreeList {
    const fn empty() -> Self {
        Self {
            head: core::ptr::null_mut(),
            count: 0,
        }
    }

    fn push(&mut self, phys: u64) {
        let virt = phys_to_virt(phys) as *mut FreeBlock;
        unsafe {
            (*virt).next = self.head;
            self.head = virt;
        }
        self.count += 1;
    }

    fn pop(&mut self) -> Option<u64> {
        if self.head.is_null() {
            return None;
        }
        unsafe {
            let block = self.head;
            self.head = (*block).next;
            self.count -= 1;
            Some(phys_to_virt_rev(block as u64))
        }
    }

    fn remove(&mut self, phys: u64) -> bool {
        let target_virt = phys_to_virt(phys) as *mut FreeBlock;
        let mut cur = &mut self.head as *mut *mut FreeBlock;
        unsafe {
            while !(*cur).is_null() {
                if *cur == target_virt {
                    *cur = (**cur).next;
                    self.count -= 1;
                    return true;
                }
                cur = &mut (**cur).next as *mut *mut FreeBlock;
            }
        }
        false
    }
}

fn phys_to_virt_rev(virt: u64) -> u64 {
    use crate::arch::x86_64::limine::virt_to_phys;
    virt_to_phys(virt)
}

struct BuddyAllocator {
    lists: [FreeList; MAX_ORDER + 1],
    base_phys: u64,
    total_frames: usize,
}

unsafe impl Send for BuddyAllocator {}

impl BuddyAllocator {
    const fn new() -> Self {
        const EMPTY: FreeList = FreeList::empty();
        Self {
            lists: [EMPTY; MAX_ORDER + 1],
            base_phys: 0,
            total_frames: 0,
        }
    }

    fn add_region(&mut self, base: u64, size: u64) {
        let mut addr = align_up(base, PAGE_SIZE as u64);
        let end = align_down(base + size, PAGE_SIZE as u64);

        while addr < end {
            let mut order = MAX_ORDER;
            loop {
                let block_size = (PAGE_SIZE << order) as u64;
                if order == 0 || addr % block_size != 0 || addr + block_size > end {
                    if order == 0 {
                        self.lists[0].push(addr);
                        FREE_PAGES.fetch_add(1, Ordering::Relaxed);
                        TOTAL_PAGES.fetch_add(1, Ordering::Relaxed);
                        addr += PAGE_SIZE as u64;
                        break;
                    }
                    order -= 1;
                    continue;
                }
                self.lists[order].push(addr);
                FREE_PAGES.fetch_add(1 << order, Ordering::Relaxed);
                TOTAL_PAGES.fetch_add(1 << order, Ordering::Relaxed);
                addr += block_size;
                break;
            }
        }
    }

    fn alloc(&mut self, order: usize) -> Option<u64> {
        let mut found_order = None;
        for o in order..=MAX_ORDER {
            if !self.lists[o].head.is_null() {
                found_order = Some(o);
                break;
            }
        }

        let found_order = found_order?;
        let phys = self.lists[found_order].pop()?;

        let mut current_order = found_order;
        while current_order > order {
            current_order -= 1;
            let buddy = phys + (PAGE_SIZE << current_order) as u64;
            self.lists[current_order].push(buddy);
        }

        FREE_PAGES.fetch_sub(1 << order, Ordering::Relaxed);
        Some(phys)
    }

    fn free(&mut self, phys: u64, order: usize) {
        let mut current_phys = phys;
        let mut current_order = order;

        loop {
            if current_order >= MAX_ORDER {
                break;
            }

            let block_size = (PAGE_SIZE << current_order) as u64;
            let buddy_phys = current_phys ^ block_size;

            if self.lists[current_order].remove(buddy_phys) {
                current_phys = current_phys.min(buddy_phys);
                current_order += 1;
            } else {
                break;
            }
        }

        self.lists[current_order].push(current_phys);
        FREE_PAGES.fetch_add(1 << order, Ordering::Relaxed);
    }
}

static PMM: SpinLock<BuddyAllocator> = SpinLock::new(BuddyAllocator::new());

pub fn init() {
    let resp = MEMMAP_REQUEST.response.load(Ordering::Relaxed);
    assert!(!resp.is_null(), "Limine memory map response is null");

    let mut pmm = PMM.lock();

    unsafe {
        let entries = (*resp).entries();
        let mut usable_bytes = 0u64;

        for &entry_ptr in entries {
            let entry = &*entry_ptr;
            log::trace!(
                "Memory: base={:#012x} len={:#012x} type={:?}",
                entry.base,
                entry.length,
                entry.kind
            );

            if entry.kind == MemoryMapEntryType::Usable {
                let base = if entry.base < 0x20_0000 {
                    let skip = 0x20_0000 - entry.base;
                    if skip >= entry.length {
                        continue;
                    }
                    entry.base + skip
                } else {
                    entry.base
                };

                let length = entry.length.saturating_sub(base - entry.base);
                if length > 0 {
                    pmm.add_region(base, length);
                    usable_bytes += length;
                }
            }
        }

        log::info!(
            "PMM: {:.1} MiB usable ({} pages)",
            usable_bytes as f64 / 1024.0 / 1024.0,
            FREE_PAGES.load(Ordering::Relaxed)
        );
    }
}

pub fn alloc_frame() -> Option<u64> {
    PMM.lock().alloc(0)
}

pub fn alloc_frames(order: usize) -> Option<u64> {
    PMM.lock().alloc(order)
}

pub fn free_frame(phys: u64) {
    PMM.lock().free(phys, 0);
}

pub fn free_frames(phys: u64, order: usize) {
    PMM.lock().free(phys, order);
}

pub fn alloc_zeroed_frame() -> Option<u64> {
    let phys = alloc_frame()?;
    let virt = crate::arch::x86_64::limine::phys_to_virt(phys) as *mut u8;
    unsafe { virt.write_bytes(0, PAGE_SIZE) };
    Some(phys)
}

pub fn free_pages() -> usize {
    FREE_PAGES.load(Ordering::Relaxed)
}

pub fn total_pages() -> usize {
    TOTAL_PAGES.load(Ordering::Relaxed)
}

pub fn used_pages() -> usize {
    total_pages().saturating_sub(free_pages())
}

#[inline]
pub fn align_up(val: u64, align: u64) -> u64 {
    (val + align - 1) & !(align - 1)
}

#[inline]
pub fn align_down(val: u64, align: u64) -> u64 {
    val & !(align - 1)
}

#[inline]
pub fn is_aligned(val: u64, align: u64) -> bool {
    val & (align - 1) == 0
}
