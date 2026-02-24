use crate::arch::x86_64::limine::phys_to_virt;
use crate::mm::pmm::{align_down, align_up, alloc_zeroed_frame, PAGE_SIZE};
use crate::mm::vmm::{
    AddressSpace, VmSpace, VmaFlags, PTE_NO_EXEC, PTE_PRESENT, PTE_USER, PTE_WRITABLE,
};
use alloc::vec::Vec;

pub type Elf64Addr = u64;
pub type Elf64Off = u64;
pub type Elf64Half = u16;
pub type Elf64Word = u32;
pub type Elf64Xword = u64;

const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];

const ELFCLASS64: u8 = 2;

const ELFDATA2LSB: u8 = 1;

const ET_EXEC: Elf64Half = 2;
const ET_DYN: Elf64Half = 3;

const EM_X86_64: Elf64Half = 62;

const PT_NULL: Elf64Word = 0;
const PT_LOAD: Elf64Word = 1;
const PT_DYNAMIC: Elf64Word = 2;
const PT_INTERP: Elf64Word = 3;
const PT_NOTE: Elf64Word = 4;
const PT_PHDR: Elf64Word = 6;
const PT_TLS: Elf64Word = 7;
const PT_GNU_STACK: Elf64Word = 0x6474_E551;
const PT_GNU_RELRO: Elf64Word = 0x6474_E552;

const PF_X: Elf64Word = 1 << 0;
const PF_W: Elf64Word = 1 << 1;
const PF_R: Elf64Word = 1 << 2;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; 16],
    pub e_type: Elf64Half,
    pub e_machine: Elf64Half,
    pub e_version: Elf64Word,
    pub e_entry: Elf64Addr,
    pub e_phoff: Elf64Off,
    pub e_shoff: Elf64Off,
    pub e_flags: Elf64Word,
    pub e_ehsize: Elf64Half,
    pub e_phentsize: Elf64Half,
    pub e_phnum: Elf64Half,
    pub e_shentsize: Elf64Half,
    pub e_shnum: Elf64Half,
    pub e_shstrndx: Elf64Half,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Phdr {
    pub p_type: Elf64Word,
    pub p_flags: Elf64Word,
    pub p_offset: Elf64Off,
    pub p_vaddr: Elf64Addr,
    pub p_paddr: Elf64Addr,
    pub p_filesz: Elf64Xword,
    pub p_memsz: Elf64Xword,
    pub p_align: Elf64Xword,
}

#[derive(Debug)]
pub struct LoadedElf {
    pub entry: u64,
    pub brk: u64,
    pub phdr_vaddr: u64,
    pub phnum: u16,
    pub phent: u16,
    pub load_base: u64,
    pub interp_path: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    NotElf64,
    NotLittleEndian,
    BadVersion,
    NotExecutable,
    WrongArch,
    BadPhdr,
    OutOfBounds,
    MappingFailed,
    AllocFailed,
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn load_elf(
    data: &[u8],
    addr_space: &mut AddressSpace,
    vm: &mut VmSpace,
    pie_base: u64,
) -> Result<LoadedElf, ElfError> {
    if data.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }

    let ehdr: &Elf64Ehdr = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    if &ehdr.e_ident[0..4] != &ELFMAG {
        return Err(ElfError::BadMagic);
    }
    if ehdr.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }
    if ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    if ehdr.e_ident[6] != 1 {
        return Err(ElfError::BadVersion);
    }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(ElfError::NotExecutable);
    }
    if ehdr.e_machine != EM_X86_64 {
        return Err(ElfError::WrongArch);
    }

    let is_pie = ehdr.e_type == ET_DYN;
    let slide = if is_pie { pie_base } else { 0 };

    let phoff = ehdr.e_phoff as usize;
    let phnum = ehdr.e_phnum as usize;
    let phentsize = ehdr.e_phentsize as usize;

    if phoff + phnum * phentsize > data.len() {
        return Err(ElfError::OutOfBounds);
    }

    let phdrs: &[Elf64Phdr] =
        unsafe { core::slice::from_raw_parts(data.as_ptr().add(phoff) as *const Elf64Phdr, phnum) };

    let mut load_min = u64::MAX;
    let mut load_max = 0u64;
    let mut phdr_vaddr = 0u64;
    let mut interp_path: Option<Vec<u8>> = None;
    let mut stack_exec = false;

    for phdr in phdrs {
        match phdr.p_type {
            PT_LOAD => {
                if phdr.p_vaddr < load_min {
                    load_min = phdr.p_vaddr;
                }
                let end = phdr.p_vaddr + phdr.p_memsz;
                if end > load_max {
                    load_max = end;
                }
            }
            PT_PHDR => {
                phdr_vaddr = phdr.p_vaddr + slide;
            }
            PT_INTERP => {
                let off = phdr.p_offset as usize;
                let sz = phdr.p_filesz as usize;
                if off + sz > data.len() {
                    return Err(ElfError::OutOfBounds);
                }
                let path = data[off..off + sz].to_vec();
                interp_path = Some(path);
            }
            PT_GNU_STACK => {
                stack_exec = phdr.p_flags & PF_X != 0;
            }
            _ => {}
        }
    }

    let load_base = if is_pie { pie_base } else { load_min };

    for phdr in phdrs {
        if phdr.p_type != PT_LOAD {
            continue;
        }

        if phdr.p_memsz == 0 {
            continue;
        }

        let seg_vaddr = phdr.p_vaddr + slide;

        let page_vaddr = align_down(seg_vaddr, PAGE_SIZE as u64);
        let page_end = align_up(seg_vaddr + phdr.p_memsz, PAGE_SIZE as u64);
        let page_count = ((page_end - page_vaddr) / PAGE_SIZE as u64) as usize;

        let mut pte_flags = PTE_PRESENT | PTE_USER;
        if phdr.p_flags & PF_W != 0 {
            pte_flags |= PTE_WRITABLE;
        }
        if phdr.p_flags & PF_X == 0 {
            pte_flags |= PTE_NO_EXEC;
        }

        let mut vma_flags = VmaFlags::empty();
        if phdr.p_flags & PF_R != 0 {
            vma_flags |= VmaFlags::READ;
        }
        if phdr.p_flags & PF_W != 0 {
            vma_flags |= VmaFlags::WRITE;
        }
        if phdr.p_flags & PF_X != 0 {
            vma_flags |= VmaFlags::EXEC;
        }

        let mut page_offset = 0u64;
        while page_offset < (page_end - page_vaddr) {
            let frame_phys = alloc_zeroed_frame().ok_or(ElfError::AllocFailed)?;

            let vaddr = page_vaddr + page_offset;
            if !addr_space.map(vaddr, frame_phys, pte_flags) {
                return Err(ElfError::MappingFailed);
            }

            page_offset += PAGE_SIZE as u64;
        }

        vm.add_vma(page_vaddr, page_end, vma_flags);

        let file_offset = phdr.p_offset as usize;
        let file_size = phdr.p_filesz as usize;

        if file_size > 0 {
            if file_offset + file_size > data.len() {
                return Err(ElfError::OutOfBounds);
            }

            let page_inner_offset = (seg_vaddr - page_vaddr) as usize;

            let src = &data[file_offset..file_offset + file_size];
            let mut bytes_copied = 0usize;

            while bytes_copied < file_size {
                let vaddr = seg_vaddr + bytes_copied as u64;
                let phys = addr_space.translate(vaddr).ok_or(ElfError::MappingFailed)?;

                let page_remaining = PAGE_SIZE - (vaddr as usize % PAGE_SIZE);
                let to_copy = (file_size - bytes_copied).min(page_remaining);

                let dst_ptr = phys_to_virt(phys) as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr().add(bytes_copied),
                        dst_ptr,
                        to_copy,
                    );
                }
                bytes_copied += to_copy;
            }
        }

        log::trace!(
            "ELF: loaded segment vaddr={:#x} filesz={:#x} memsz={:#x} flags={:03b}",
            seg_vaddr,
            phdr.p_filesz,
            phdr.p_memsz,
            phdr.p_flags & 7
        );
    }

    let brk = align_up(load_max + slide, PAGE_SIZE as u64);
    vm.brk = brk;

    if phdr_vaddr == 0 && phoff != 0 {
        for phdr in phdrs {
            if phdr.p_type == PT_LOAD && phdr.p_offset == 0 {
                phdr_vaddr = phdr.p_vaddr + slide + ehdr.e_phoff;
                break;
            }
        }
    }

    let entry = ehdr.e_entry + slide;

    log::debug!(
        "ELF loaded: entry={:#018x} brk={:#018x} pie={} interp={}",
        entry,
        brk,
        is_pie,
        interp_path.is_some()
    );

    Ok(LoadedElf {
        entry,
        brk,
        phdr_vaddr,
        phnum: ehdr.e_phnum,
        phent: ehdr.e_phentsize,
        load_base,
        interp_path,
    })
}

pub fn is_valid_elf(data: &[u8]) -> bool {
    if data.len() < core::mem::size_of::<Elf64Ehdr>() {
        return false;
    }
    let ehdr = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };
    &ehdr.e_ident[0..4] == &ELFMAG
        && ehdr.e_ident[4] == ELFCLASS64
        && ehdr.e_ident[5] == ELFDATA2LSB
        && ehdr.e_machine == EM_X86_64
}

pub fn read_cstr(data: &[u8], offset: usize) -> &[u8] {
    let slice = &data[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    &slice[..end]
}
