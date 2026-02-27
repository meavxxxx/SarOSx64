use crate::arch::x86_64::limine::phys_to_virt;
use crate::mm::pmm::PAGE_SIZE;
use crate::mm::pmm::{align_down, align_up, alloc_zeroed_frame};
use crate::mm::vmm::{
    AddressSpace, VmSpace, VmaFlags, PTE_NO_EXEC, PTE_PRESENT, PTE_USER, PTE_WRITABLE,
};
use crate::proc::elf::LoadedElf;
use alloc::vec::Vec;

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_NOTELF: u64 = 10;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;

pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
pub const USER_STACK_SIZE: u64 = 8 * 1024 * 1024;
pub const USER_STACK_BOTTOM: u64 = USER_STACK_TOP - USER_STACK_SIZE;

pub struct StackBuilder {
    kernel_ptr: u64,
    user_ptr: u64,
    size: u64,
    phys_base: u64,
}

impl StackBuilder {
    fn new() -> Option<Self> {
        let size = USER_STACK_SIZE;
        let phys_base = alloc_zeroed_frame()?;
        let phys_base = alloc_stack_frames(size)?;

        let kernel_base = phys_to_virt(phys_base);

        Some(Self {
            kernel_ptr: kernel_base + size,
            user_ptr: USER_STACK_TOP,
            size,
            phys_base,
        })
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> u64 {
        let len = bytes.len();
        self.kernel_ptr -= len as u64;
        self.user_ptr -= len as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.kernel_ptr as *mut u8, len);
        }
        self.user_ptr
    }

    fn push_str(&mut self, s: &[u8]) -> u64 {
        self.push_bytes(&[0]);
        self.push_bytes(s)
    }

    fn align(&mut self, align: u64) {
        let misalign = self.user_ptr % align;
        if misalign != 0 {
            let pad = align - misalign;
            self.kernel_ptr -= pad;
            self.user_ptr -= pad;
        }
    }

    fn push_u64(&mut self, val: u64) -> u64 {
        self.kernel_ptr -= 8;
        self.user_ptr -= 8;
        unsafe {
            *(self.kernel_ptr as *mut u64) = val;
        }
        self.user_ptr
    }
}

fn alloc_stack_frames(size: u64) -> Option<u64> {
    alloc_zeroed_frame()
}

pub struct UserStack {
    pub initial_rsp: u64,
}

pub fn build_user_stack(
    addr_space: &mut AddressSpace,
    vm: &mut VmSpace,
    loaded: &LoadedElf,
    at_base: u64,
    argv: &[&[u8]],
    envp: &[&[u8]],
    execfn: &[u8],
) -> Option<UserStack> {
    map_user_stack(addr_space, vm)?;

    let mut stack = Vec::<u64>::new();

    let mut cursor = USER_STACK_TOP;

    let write_at = |addr_space: &AddressSpace, virt: u64, data: &[u8]| {
        let mut offset = 0;
        while offset < data.len() {
            let v = virt + offset as u64;
            let phys = addr_space.translate(v)?;
            let page_off = (v % PAGE_SIZE as u64) as usize;
            let avail = PAGE_SIZE - page_off;
            let to_copy = avail.min(data.len() - offset);
            unsafe {
                let dst = phys_to_virt(phys) as *mut u8;
                core::ptr::copy_nonoverlapping(data.as_ptr().add(offset), dst, to_copy);
            }
            offset += to_copy;
        }
        Some(())
    };

    cursor -= 16;
    let at_random_ptr = cursor;
    let random_bytes = [
        0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x13, 0x37, 0x42, 0x00, 0x11, 0x22, 0x33,
        0x44u8,
    ];
    write_at(addr_space, cursor, &random_bytes)?;

    cursor -= (execfn.len() + 1) as u64;
    let execfn_ptr = cursor;
    write_at(addr_space, cursor, execfn)?;

    let mut argv_ptrs = Vec::with_capacity(argv.len());
    for arg in argv.iter().rev() {
        cursor -= (arg.len() + 1) as u64;
        write_at(addr_space, cursor, arg)?;
        argv_ptrs.push(cursor);
    }
    argv_ptrs.reverse();

    let mut envp_ptrs = Vec::with_capacity(envp.len());
    for env in envp.iter().rev() {
        cursor -= (env.len() + 1) as u64;
        write_at(addr_space, cursor, env)?;
        envp_ptrs.push(cursor);
    }
    envp_ptrs.reverse();
    cursor &= !15u64;
    let mut rsp = cursor;

    macro_rules! push {
        ($val:expr) => {{
            rsp -= 8;
            write_at(addr_space, rsp, &($val as u64).to_le_bytes())?;
        }};
    }

    push!(0u64);
    push!(AT_NULL);

    push!(execfn_ptr);
    push!(AT_EXECFN);

    push!(at_random_ptr);
    push!(AT_RANDOM);

    push!(loaded.entry);
    push!(AT_ENTRY);

    push!(0u64);
    push!(AT_FLAGS);

    push!(at_base);
    push!(AT_BASE);

    push!(PAGE_SIZE as u64);
    push!(AT_PAGESZ);

    push!(loaded.phnum as u64);
    push!(AT_PHNUM);

    push!(loaded.phent as u64);
    push!(AT_PHENT);

    push!(loaded.phdr_vaddr);
    push!(AT_PHDR);

    push!(0u64);
    push!(AT_EGID);
    push!(0u64);
    push!(AT_GID);
    push!(0u64);
    push!(AT_EUID);
    push!(0u64);
    push!(AT_UID);

    push!(0u64);
    for &ptr in envp_ptrs.iter().rev() {
        push!(ptr);
    }

    push!(0u64);
    for &ptr in argv_ptrs.iter().rev() {
        push!(ptr);
    }

    push!(argv.len() as u64);

    log::debug!("User stack built: rsp={:#018x}", rsp);

    Some(UserStack { initial_rsp: rsp })
}

fn map_user_stack(addr_space: &mut AddressSpace, vm: &mut VmSpace) -> Option<()> {
    let stack_flags = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWS_DOWN | VmaFlags::ANONYMOUS;
    vm.add_vma(USER_STACK_BOTTOM, USER_STACK_TOP, stack_flags);

    let pte_flags = PTE_PRESENT | PTE_WRITABLE | PTE_USER | PTE_NO_EXEC;

    let initial_committed = 64 * 1024u64;
    let commit_start = USER_STACK_TOP - initial_committed;

    let mut vaddr = commit_start;
    while vaddr < USER_STACK_TOP {
        let phys = alloc_zeroed_frame()?;
        addr_space.map(vaddr, phys, pte_flags);
        vaddr += PAGE_SIZE as u64;
    }

    Some(())
}
