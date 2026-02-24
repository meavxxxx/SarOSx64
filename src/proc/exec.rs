use crate::arch::x86_64::gdt::{set_kernel_stack, SEG_USER_CODE, SEG_USER_DATA};
use crate::arch::x86_64::idt::InterruptFrame;
use crate::mm::pmm::PAGE_SIZE;
use crate::mm::vmm::{AddressSpace, VmSpace, PTE_NO_EXEC, PTE_PRESENT, PTE_USER, PTE_WRITABLE};
use crate::proc::elf::{load_elf, ElfError, LoadedElf};
use crate::proc::stack::{build_user_stack, UserStack, USER_STACK_TOP};
use alloc::vec::Vec;

const PIE_BASE: u64 = 0x0000_5555_5555_0000;

const INTERP_BASE: u64 = 0x0000_7FFF_0000_0000;

#[derive(Debug)]
pub enum ExecError {
    NotFound,
    Permission,
    NoMemory,
    ElfError(ElfError),
    StackError,
    NameTooLong,
}

impl From<ElfError> for ExecError {
    fn from(e: ElfError) -> Self {
        ExecError::ElfError(e)
    }
}

pub fn exec(
    elf_data: &[u8],
    argv: &[Vec<u8>],
    envp: &[Vec<u8>],
    path: &[u8],
) -> Result<!, ExecError> {
    let mut new_space = AddressSpace::new_user().ok_or(ExecError::NoMemory)?;
    let mut new_vm = VmSpace::new();

    let pie_base = if is_pie(elf_data) { PIE_BASE } else { 0 };

    let loaded =
        load_elf(elf_data, &mut new_space, &mut new_vm, pie_base).map_err(ExecError::ElfError)?;

    log::debug!("execve: main ELF loaded, entry={:#x}", loaded.entry);

    let actual_entry;
    let interp_loaded;

    if let Some(ref interp_path) = loaded.interp_path {
        let interp_data = load_file_from_initrd(interp_path).ok_or(ExecError::NotFound)?;

        let il = load_elf(&interp_data, &mut new_space, &mut new_vm, INTERP_BASE)
            .map_err(ExecError::ElfError)?;

        log::debug!("execve: interpreter loaded, entry={:#x}", il.entry);

        actual_entry = il.entry;
        interp_loaded = Some(il);
    } else {
        actual_entry = loaded.entry;
        interp_loaded = None;
    }

    let argv_refs: Vec<&[u8]> = argv.iter().map(|v| v.as_slice()).collect();
    let envp_refs: Vec<&[u8]> = envp.iter().map(|v| v.as_slice()).collect();

    let stack = build_user_stack(
        &mut new_space,
        &mut new_vm,
        &loaded,
        &argv_refs,
        &envp_refs,
        path,
    )
    .ok_or(ExecError::StackError)?;

    let proc_arc = crate::proc::scheduler::current_process().ok_or(ExecError::NoMemory)?;

    {
        let mut proc = proc_arc.lock();

        proc.address_space = new_space;
        proc.vm = new_vm;

        let name_len = path.len().min(31);
        proc.name = [0u8; 32];
        proc.name[..name_len].copy_from_slice(&path[..name_len]);

        let kstack_top = proc.kernel_stack + proc.kernel_stack_size as u64;
        set_kernel_stack(kstack_top);
    }

    proc_arc.lock().address_space.activate();

    log::info!(
        "execve: pid={} entry={:#018x} rsp={:#018x}",
        proc_arc.lock().pid,
        actual_entry,
        stack.initial_rsp
    );

    unsafe {
        jump_to_user(
            actual_entry,
            stack.initial_rsp,
            SEG_USER_CODE as u64,
            SEG_USER_DATA as u64,
        )
    }
}

#[naked]
unsafe extern "C" fn jump_to_user(entry: u64, user_rsp: u64, user_cs: u64, user_ss: u64) -> ! {
    core::arch::asm!(
        "push %rcx",
        "push %rsi",
        "pushfq",
        "pop %rax",
        "or $0x200, %rax",
        "and $~0x3000, %rax",
        "push %rax",
        "push %rdx",
        "push %rdi",
        "xor %rax, %rax",
        "xor %rbx, %rbx",
        "xor %rcx, %rcx",
        "xor %rdx, %rdx",
        "xor %rsi, %rsi",
        "xor %rdi, %rdi",
        "xor %rbp, %rbp",
        "xor %r8,  %r8",
        "xor %r9,  %r9",
        "xor %r10, %r10",
        "xor %r11, %r11",
        "xor %r12, %r12",
        "xor %r13, %r13",
        "xor %r14, %r14",
        "xor %r15, %r15",
        "iretq",
        options(noreturn, att_syntax)
    );
}

fn is_pie(data: &[u8]) -> bool {
    if data.len() < 18 {
        return false;
    }
    let e_type = u16::from_le_bytes([data[16], data[17]]);
    e_type == 3
}

pub fn load_file_from_initrd(path: &[u8]) -> Option<Vec<u8>> {
    log::warn!(
        "load_file_from_initrd: VFS not implemented, path={:?}",
        path
    );
    None
}

pub fn sys_execve(pathname_ptr: u64, argv_ptr: u64, envp_ptr: u64, frame: &InterruptFrame) -> i64 {
    use crate::syscall::errno::*;

    let proc_arc = match crate::proc::scheduler::current_process() {
        Some(p) => p,
        None => return -EINVAL,
    };

    let path = match read_user_string(&proc_arc.lock().address_space, pathname_ptr, 4096) {
        Some(s) => s,
        None => return -EFAULT,
    };

    log::info!(
        "execve: path={:?}",
        core::str::from_utf8(&path).unwrap_or("?")
    );

    let argv = match read_user_string_array(&proc_arc.lock().address_space, argv_ptr, 256) {
        Some(a) => a,
        None => return -EFAULT,
    };

    let envp = match read_user_string_array(&proc_arc.lock().address_space, envp_ptr, 256) {
        Some(e) => e,
        None => return -EFAULT,
    };

    let elf_data = match lookup_and_read_file(&proc_arc.lock().address_space, &path) {
        Some(d) => d,
        None => return -ENOENT,
    };

    match exec(&elf_data, &argv, &envp, &path) {
        Ok(never) => never,
        Err(ExecError::NotFound) => -ENOENT,
        Err(ExecError::Permission) => -EACCES,
        Err(ExecError::NoMemory) => -ENOMEM,
        Err(ExecError::NameTooLong) => -ENAMETOOLONG,
        Err(e) => {
            log::error!("execve failed: {:?}", e);
            -EINVAL
        }
    }
}

const ENAMETOOLONG: i64 = 36;

fn read_user_string(space: &AddressSpace, ptr: u64, max_len: usize) -> Option<Vec<u8>> {
    if ptr == 0 {
        return None;
    }

    let mut result = Vec::new();
    let mut addr = ptr;

    loop {
        if result.len() >= max_len {
            return None;
        }

        let phys = space.translate(addr)?;
        let byte = unsafe { *(phys_to_virt(phys) as *const u8) };

        if byte == 0 {
            break;
        }
        result.push(byte);
        addr += 1;
    }

    result.push(0);
    Some(result)
}

fn read_user_string_array(
    space: &AddressSpace,
    ptr: u64,
    max_count: usize,
) -> Option<Vec<Vec<u8>>> {
    if ptr == 0 {
        return Some(Vec::new());
    }

    let mut result = Vec::new();
    let mut addr = ptr;

    loop {
        if result.len() >= max_count {
            return None;
        }

        let phys = space.translate(addr)?;
        let str_ptr = unsafe { *(phys_to_virt(phys) as *const u64) };

        if str_ptr == 0 {
            break;
        }

        let s = read_user_string(space, str_ptr, 65536)?;
        result.push(s);
        addr += 8;
    }

    Some(result)
}

fn lookup_and_read_file(space: &AddressSpace, path: &[u8]) -> Option<Vec<u8>> {
    use crate::proc::exec::INITRD;
    if let Some(initrd) = unsafe { INITRD } {
        find_in_cpio(initrd, path)
    } else {
        None
    }
}

pub static mut INITRD: Option<&'static [u8]> = None;

fn find_in_cpio(cpio: &[u8], path: &[u8]) -> Option<Vec<u8>> {
    let mut offset = 0usize;
    let path_str = core::str::from_utf8(path).ok()?.trim_start_matches('/');

    while offset + 110 <= cpio.len() {
        if &cpio[offset..offset + 6] != b"070701" && &cpio[offset..offset + 6] != b"070702" {
            break;
        }

        let namesize = parse_hex8(&cpio[offset + 94..offset + 102])?;
        let filesize = parse_hex8(&cpio[offset + 54..offset + 62])?;

        let name_start = offset + 110;
        let name_end = name_start + namesize as usize;
        if name_end > cpio.len() {
            break;
        }

        let name = &cpio[name_start..name_end.saturating_sub(1)]; // без null
        let name_str = core::str::from_utf8(name).unwrap_or("");

        let data_start = align4(name_end);
        let data_end = data_start + filesize as usize;

        if name_str == "TRAILER!!!" {
            break; // Конец архива
        }

        if name_str == path_str || name_str == path_str.trim_start_matches('/') {
            if data_end <= cpio.len() {
                return Some(cpio[data_start..data_end].to_vec());
            }
        }

        offset = align4(data_end);
    }

    None
}

fn parse_hex8(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 8 {
        return None;
    }
    let s = core::str::from_utf8(&bytes[..8]).ok()?;
    u32::from_str_radix(s, 16).ok()
}

fn align4(v: usize) -> usize {
    (v + 3) & !3
}

use crate::arch::x86_64::limine::phys_to_virt;
use crate::syscall::errno::{EACCES, EFAULT, EINVAL, ENOENT};

pub fn sys_execve_simple(pathname: u64, argv_ptr: u64, envp_ptr: u64) -> i64 {
    use crate::syscall::errno::*;

    let arc = match crate::proc::scheduler::current_process() {
        Some(p) => p,
        None => return -EINVAL,
    };

    let path = match read_user_string_from(&arc.lock().address_space, pathname, 4096) {
        Some(s) => s,
        None => return -EFAULT,
    };

    log::info!("execve({:?})", core::str::from_utf8(&path).unwrap_or("?"));

    let argv = read_string_array(&arc.lock().address_space, argv_ptr, 256).unwrap_or_default();
    let envp = read_string_array(&arc.lock().address_space, envp_ptr, 256).unwrap_or_default();

    let elf_data = match lookup_and_read_file(&arc.lock().address_space, &path) {
        Some(d) => d,
        None => return -ENOENT,
    };

    match exec(&elf_data, &argv, &envp, &path) {
        Ok(never) => never,
        Err(ExecError::NotFound) => -ENOENT,
        Err(ExecError::NoMemory) => -ENOMEM,
        Err(ExecError::Permission) => -EACCES,
        Err(e) => {
            log::error!("exec failed: {:?}", e);
            -EINVAL
        }
    }
}

fn read_user_string_from(
    space: &crate::mm::vmm::AddressSpace,
    ptr: u64,
    max: usize,
) -> Option<alloc::vec::Vec<u8>> {
    if ptr == 0 {
        return None;
    }
    let mut result = alloc::vec::Vec::new();
    let mut addr = ptr;
    loop {
        if result.len() >= max {
            return None;
        }
        let phys = space.translate(addr)?;
        let byte = unsafe { *(phys_to_virt(phys) as *const u8) };
        if byte == 0 {
            break;
        }
        result.push(byte);
        addr += 1;
    }
    result.push(0);
    Some(result)
}

fn read_string_array(
    space: &crate::mm::vmm::AddressSpace,
    ptr: u64,
    max: usize,
) -> Option<alloc::vec::Vec<alloc::vec::Vec<u8>>> {
    if ptr == 0 {
        return Some(alloc::vec::Vec::new());
    }
    let mut result = alloc::vec::Vec::new();
    let mut addr = ptr;
    loop {
        if result.len() >= max {
            return None;
        }
        let phys = space.translate(addr)?;
        let str_ptr = unsafe { *(phys_to_virt(phys) as *const u64) };
        if str_ptr == 0 {
            break;
        }
        let s = read_user_string_from(space, str_ptr, 65536)?;
        result.push(s);
        addr += 8;
    }
    Some(result)
}
