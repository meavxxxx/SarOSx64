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
    let ehdr = parse_ehdr(data)?;

    if &ehdr.e_ident[0..4] != &ELFMAG {
        return Err(ElfError::BadMagic);
    }
    if ehdr.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }
    if ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    if ehdr.e_ident[6] != 1 || ehdr.e_version != 1 {
        return Err(ElfError::BadVersion);
    }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(ElfError::NotExecutable);
    }
    if ehdr.e_machine != EM_X86_64 {
        return Err(ElfError::WrongArch);
    }
    if ehdr.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::BadPhdr);
    }

    let phoff = usize::try_from(ehdr.e_phoff).map_err(|_| ElfError::OutOfBounds)?;
    let phnum = ehdr.e_phnum as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let ph_size = phnum.checked_mul(phentsize).ok_or(ElfError::BadPhdr)?;
    let ph_end = phoff.checked_add(ph_size).ok_or(ElfError::BadPhdr)?;
    if ph_end > data.len() {
        return Err(ElfError::OutOfBounds);
    }

    let phdrs = parse_phdrs(data, phoff, phnum, phentsize)?;

    let is_pie = ehdr.e_type == ET_DYN;
    let slide = if is_pie { pie_base } else { 0 };

    let mut load_min = u64::MAX;
    let mut load_max = 0u64;
    let mut phdr_vaddr = 0u64;
    let mut interp_path: Option<Vec<u8>> = None;

    for phdr in phdrs.iter() {
        match phdr.p_type {
            PT_LOAD => {
                if phdr.p_memsz == 0 {
                    continue;
                }
                if phdr.p_filesz > phdr.p_memsz {
                    return Err(ElfError::BadPhdr);
                }
                if phdr.p_align != 0 && (phdr.p_align & (phdr.p_align - 1)) != 0 {
                    return Err(ElfError::BadPhdr);
                }
                if phdr.p_align > 1
                    && (phdr.p_vaddr & (phdr.p_align - 1)) != (phdr.p_offset & (phdr.p_align - 1))
                {
                    return Err(ElfError::BadPhdr);
                }

                let end = phdr.p_vaddr.checked_add(phdr.p_memsz).ok_or(ElfError::BadPhdr)?;
                load_min = load_min.min(phdr.p_vaddr);
                load_max = load_max.max(end);

                let off = usize::try_from(phdr.p_offset).map_err(|_| ElfError::OutOfBounds)?;
                let filesz = usize::try_from(phdr.p_filesz).map_err(|_| ElfError::OutOfBounds)?;
                let file_end = off.checked_add(filesz).ok_or(ElfError::OutOfBounds)?;
                if file_end > data.len() {
                    return Err(ElfError::OutOfBounds);
                }
            }
            PT_PHDR => {
                phdr_vaddr = phdr.p_vaddr.checked_add(slide).ok_or(ElfError::BadPhdr)?;
            }
            PT_INTERP => {
                let off = usize::try_from(phdr.p_offset).map_err(|_| ElfError::OutOfBounds)?;
                let sz = usize::try_from(phdr.p_filesz).map_err(|_| ElfError::OutOfBounds)?;
                let end = off.checked_add(sz).ok_or(ElfError::OutOfBounds)?;
                if end > data.len() || sz == 0 {
                    return Err(ElfError::OutOfBounds);
                }
                interp_path = Some(data[off..end].to_vec());
            }
            PT_GNU_STACK | PT_GNU_RELRO | PT_DYNAMIC | PT_NOTE | PT_NULL | PT_TLS => {}
            _ => {}
        }
    }

    if load_min == u64::MAX {
        return Err(ElfError::BadPhdr);
    }

    // For ET_DYN this is the chosen slide/base; for ET_EXEC keep 0 (AT_BASE).
    let load_base = if is_pie { pie_base } else { 0 };

    for phdr in phdrs.iter() {
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }

        let seg_vaddr = phdr.p_vaddr.checked_add(slide).ok_or(ElfError::BadPhdr)?;
        let seg_end = seg_vaddr.checked_add(phdr.p_memsz).ok_or(ElfError::BadPhdr)?;

        let page_vaddr = align_down(seg_vaddr, PAGE_SIZE as u64);
        let page_end = align_up(seg_end, PAGE_SIZE as u64);

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

        let mut page = page_vaddr;
        while page < page_end {
            if addr_space.translate(page).is_none() {
                let frame_phys = alloc_zeroed_frame().ok_or(ElfError::AllocFailed)?;
                if !addr_space.map(page, frame_phys, pte_flags) {
                    return Err(ElfError::MappingFailed);
                }
            }
            page = page
                .checked_add(PAGE_SIZE as u64)
                .ok_or(ElfError::MappingFailed)?;
        }

        vm.add_vma(page_vaddr, page_end, vma_flags);

        let file_size = usize::try_from(phdr.p_filesz).map_err(|_| ElfError::OutOfBounds)?;
        if file_size > 0 {
            let file_offset = usize::try_from(phdr.p_offset).map_err(|_| ElfError::OutOfBounds)?;
            let end = file_offset
                .checked_add(file_size)
                .ok_or(ElfError::OutOfBounds)?;
            if end > data.len() {
                return Err(ElfError::OutOfBounds);
            }

            let src = &data[file_offset..end];
            let mut copied = 0usize;
            while copied < src.len() {
                let vaddr = seg_vaddr
                    .checked_add(copied as u64)
                    .ok_or(ElfError::MappingFailed)?;
                let phys = addr_space.translate(vaddr).ok_or(ElfError::MappingFailed)?;
                let page_remaining = PAGE_SIZE - (vaddr as usize % PAGE_SIZE);
                let to_copy = (src.len() - copied).min(page_remaining);

                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr().add(copied),
                        phys_to_virt(phys) as *mut u8,
                        to_copy,
                    );
                }
                copied += to_copy;
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

    if phdr_vaddr == 0 && ehdr.e_phoff != 0 {
        let ph_bytes = (ehdr.e_phnum as u64)
            .checked_mul(ehdr.e_phentsize as u64)
            .ok_or(ElfError::BadPhdr)?;
        for phdr in phdrs.iter() {
            if phdr.p_type != PT_LOAD || phdr.p_filesz == 0 {
                continue;
            }
            let seg_file_start = phdr.p_offset;
            let seg_file_end = phdr
                .p_offset
                .checked_add(phdr.p_filesz)
                .ok_or(ElfError::BadPhdr)?;
            let ph_file_end = ehdr.e_phoff.checked_add(ph_bytes).ok_or(ElfError::BadPhdr)?;
            if ehdr.e_phoff >= seg_file_start && ph_file_end <= seg_file_end {
                let delta = ehdr.e_phoff - seg_file_start;
                phdr_vaddr = phdr
                    .p_vaddr
                    .checked_add(slide)
                    .and_then(|v| v.checked_add(delta))
                    .ok_or(ElfError::BadPhdr)?;
                break;
            }
        }
    }

    let entry = ehdr.e_entry.checked_add(slide).ok_or(ElfError::BadPhdr)?;
    let brk = align_up(
        load_max.checked_add(slide).ok_or(ElfError::BadPhdr)?,
        PAGE_SIZE as u64,
    );
    vm.brk = brk;

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
    if data.len() < 20 {
        return false;
    }
    let e_type = u16::from_le_bytes([data[16], data[17]]);
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    &data[0..4] == &ELFMAG
        && data[4] == ELFCLASS64
        && data[5] == ELFDATA2LSB
        && (e_type == ET_EXEC || e_type == ET_DYN)
        && e_machine == EM_X86_64
}

pub fn read_cstr(data: &[u8], offset: usize) -> &[u8] {
    if offset >= data.len() {
        return &[];
    }
    let slice = &data[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    &slice[..end]
}

fn parse_ehdr(data: &[u8]) -> Result<Elf64Ehdr, ElfError> {
    if data.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }
    Ok(unsafe { core::ptr::read_unaligned(data.as_ptr() as *const Elf64Ehdr) })
}

fn parse_phdrs(
    data: &[u8],
    phoff: usize,
    phnum: usize,
    phentsize: usize,
) -> Result<Vec<Elf64Phdr>, ElfError> {
    if phentsize != core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::BadPhdr);
    }
    let mut phdrs = Vec::with_capacity(phnum);
    for i in 0..phnum {
        let off = phoff
            .checked_add(i.checked_mul(phentsize).ok_or(ElfError::BadPhdr)?)
            .ok_or(ElfError::BadPhdr)?;
        let end = off.checked_add(phentsize).ok_or(ElfError::BadPhdr)?;
        if end > data.len() {
            return Err(ElfError::OutOfBounds);
        }
        let phdr = unsafe { core::ptr::read_unaligned(data.as_ptr().add(off) as *const Elf64Phdr) };
        phdrs.push(phdr);
    }
    Ok(phdrs)
}
