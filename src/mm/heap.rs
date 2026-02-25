use crate::arch::x86_64::limine::phys_to_virt;
use crate::mm::pmm::{align_up, alloc_frames, alloc_zeroed_frame, free_frame, free_frames, PAGE_SIZE};
use crate::sync::spinlock::SpinLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

const SLAB_SIZES: [usize; 9] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048];
const NUM_SLABS: usize = SLAB_SIZES.len();

struct FreeObj {
    next: *mut FreeObj,
}

struct Slab {
    base: *mut u8,
    obj_size: usize,
    free: *mut FreeObj,
    used: usize,
    capacity: usize,
    next: *mut Slab,
}

impl Slab {
    unsafe fn new(obj_size: usize) -> Option<*mut Slab> {
        let phys = alloc_zeroed_frame()?;
        let base = phys_to_virt(phys) as *mut u8;

        let slab = base as *mut Slab;
        let header_size = align_up(core::mem::size_of::<Slab>() as u64, obj_size as u64) as usize;
        let capacity = (PAGE_SIZE - header_size) / obj_size;

        (*slab).base = base;
        (*slab).obj_size = obj_size;
        (*slab).free = core::ptr::null_mut();
        (*slab).used = 0;
        (*slab).capacity = capacity;
        (*slab).next = core::ptr::null_mut();

        let objs_start = base.add(header_size);
        let mut prev: *mut FreeObj = core::ptr::null_mut();
        for i in (0..capacity).rev() {
            let obj = objs_start.add(i * obj_size) as *mut FreeObj;
            (*obj).next = prev;
            prev = obj;
        }
        (*slab).free = prev;

        Some(slab)
    }

    unsafe fn alloc(&mut self) -> Option<*mut u8> {
        if self.free.is_null() {
            return None;
        }
        let obj = self.free;
        self.free = (*obj).next;
        self.used += 1;
        Some(obj as *mut u8)
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        let obj = ptr as *mut FreeObj;
        (*obj).next = self.free;
        self.free = obj;
        self.used -= 1;
    }

    fn is_empty(&self) -> bool {
        self.used == 0
    }

    fn is_full(&self) -> bool {
        self.used == self.capacity
    }
}

struct SlabCache {
    obj_size: usize,
    partial: *mut Slab,
    full: *mut Slab,
}

unsafe impl Send for SlabCache {}

impl SlabCache {
    const fn new(obj_size: usize) -> Self {
        Self {
            obj_size,
            partial: core::ptr::null_mut(),
            full: core::ptr::null_mut(),
        }
    }

    unsafe fn alloc(&mut self) -> Option<*mut u8> {
        if !self.partial.is_null() {
            let slab = self.partial;
            let ptr = (*slab).alloc()?;

            if (*slab).is_full() {
                self.partial = (*slab).next;
                (*slab).next = self.full;
                self.full = slab;
            }

            return Some(ptr);
        }

        let slab = Slab::new(self.obj_size)?;
        (*slab).next = self.partial;
        self.partial = slab;

        self.alloc()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        let page_base = (ptr as usize) & !(PAGE_SIZE - 1);
        let slab = page_base as *mut Slab;

        let was_full = (*slab).is_full();
        (*slab).dealloc(ptr);

        if was_full {
            self.remove_from_full(slab);
            (*slab).next = self.partial;
            self.partial = slab;
        } else if (*slab).is_empty() {
            self.remove_from_partial(slab);
            free_frame(crate::arch::x86_64::limine::virt_to_phys(
                (*slab).base as u64,
            ));
        }
    }

    unsafe fn remove_from_full(&mut self, target: *mut Slab) {
        let mut cur = &mut self.full as *mut *mut Slab;
        while !(*cur).is_null() {
            if *cur == target {
                *cur = (*target).next;
                return;
            }
            cur = &mut (**cur).next as *mut *mut Slab;
        }
    }

    unsafe fn remove_from_partial(&mut self, target: *mut Slab) {
        let mut cur = &mut self.partial as *mut *mut Slab;
        while !(*cur).is_null() {
            if *cur == target {
                *cur = (*target).next;
                return;
            }
            cur = &mut (**cur).next as *mut *mut Slab;
        }
    }
}

struct KernelAllocator {
    caches: [SlabCache; NUM_SLABS],
}

unsafe impl Send for KernelAllocator {}

impl KernelAllocator {
    const fn new() -> Self {
        Self {
            caches: [
                SlabCache::new(8),
                SlabCache::new(16),
                SlabCache::new(32),
                SlabCache::new(64),
                SlabCache::new(128),
                SlabCache::new(256),
                SlabCache::new(512),
                SlabCache::new(1024),
                SlabCache::new(2048),
            ],
        }
    }

    fn find_cache(&mut self, size: usize, align: usize) -> Option<&mut SlabCache> {
        let need = size.max(align);
        for cache in &mut self.caches {
            if cache.obj_size >= need {
                return Some(cache);
            }
        }
        None
    }
}

static ALLOCATOR: SpinLock<KernelAllocator> = SpinLock::new(KernelAllocator::new());

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();

        if size == 0 {
            return align as *mut u8;
        } // ZST

        let mut alloc = ALLOCATOR.lock();

        if size <= 2048 && align <= 2048 {
            if let Some(cache) = alloc.find_cache(size, align) {
                return cache.alloc().unwrap_or(core::ptr::null_mut());
            }
        }

        let pages = (align_up(size as u64, PAGE_SIZE as u64) / PAGE_SIZE as u64) as usize;
        let order = usize::BITS as usize - pages.next_power_of_two().leading_zeros() as usize - 1;
        match alloc_frames(order) {
            Some(phys) => {
                let virt = phys_to_virt(phys) as *mut u8;
                unsafe { core::ptr::write_bytes(virt, 0, (1 << order) * PAGE_SIZE) };
                virt
            }
            None => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }

        let size = layout.size();
        let align = layout.align();

        if size <= 2048 && align <= 2048 {
            let mut alloc = ALLOCATOR.lock();
            if let Some(cache) = alloc.find_cache(size, align) {
                cache.dealloc(ptr);
                return;
            }
        }

        let pages = (align_up(size as u64, PAGE_SIZE as u64) / PAGE_SIZE as u64) as usize;
        let order = usize::BITS as usize - pages.next_power_of_two().leading_zeros() as usize - 1;
        let phys = crate::arch::x86_64::limine::virt_to_phys(ptr as u64);
        free_frames(phys, order);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = Layout::from_size_align(new_size, layout.align()).unwrap();
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            let copy_size = layout.size().min(new_size);
            core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

#[global_allocator]
pub static HEAP: KernelHeap = KernelHeap;

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    panic!("Kernel OOM: failed to allocate {:?}", layout);
}
