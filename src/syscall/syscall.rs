pub mod nr {
    pub const SYS_READ: u64 = 0;
    pub const SYS_WRITE: u64 = 1;
    pub const SYS_OPEN: u64 = 2;
    pub const SYS_CLOSE: u64 = 3;
    pub const SYS_MMAP: u64 = 9;
    pub const SYS_MUNMAP: u64 = 11;
    pub const SYS_BRK: u64 = 12;
    pub const SYS_SIGACTION: u64 = 13;
    pub const SYS_SIGPROCMASK: u64 = 14;
    pub const SYS_IOCTL: u64 = 16;
    pub const SYS_FORK: u64 = 57;
    pub const SYS_VFORK: u64 = 58;
    pub const SYS_EXECVE: u64 = 59;
    pub const SYS_EXIT: u64 = 60;
    pub const SYS_WAIT4: u64 = 61;
    pub const SYS_KILL: u64 = 62;
    pub const SYS_UNAME: u64 = 63;
    pub const SYS_GETPID: u64 = 39;
    pub const SYS_GETPPID: u64 = 110;
    pub const SYS_GETUID: u64 = 102;
    pub const SYS_GETGID: u64 = 104;
    pub const SYS_GETTID: u64 = 186;
    pub const SYS_SET_TID_ADDRESS: u64 = 218;
    pub const SYS_EXIT_GROUP: u64 = 231;
    pub const SYS_CLOCK_GETTIME: u64 = 228;
}

pub mod errno {
    pub const ENOSYS: i64 = 38;
    pub const EINVAL: i64 = 22;
    pub const EBADF: i64 = 9;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const EACCES: i64 = 13;
    pub const ENOENT: i64 = 2;
    pub const EEXIST: i64 = 17;
    pub const EAGAIN: i64 = 11;
    pub const EPERM: i64 = 1;
    pub const ECHILD: i64 = 10;
    pub const ESRCH: i64 = 3;
}

use crate::arch::x86_64::idt::InterruptFrame;
use errno::*;

#[no_mangle]
pub extern "C" fn syscall_dispatch(
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> i64 {
    use nr::*;
    match nr {
        SYS_READ => fs::sys_read(a0 as i32, a1 as *mut u8, a2 as usize),
        SYS_WRITE => fs::sys_write(a0 as i32, a1 as *const u8, a2 as usize),
        SYS_OPEN => -ENOSYS,
        SYS_CLOSE => {
            if a0 <= 2 {
                0
            } else {
                -EBADF
            }
        }
        SYS_FORK | SYS_VFORK => crate::proc::fork::sys_fork_simple(),
        SYS_EXECVE => crate::proc::exec::sys_execve_simple(a0, a1, a2),
        SYS_EXIT | SYS_EXIT_GROUP => {
            if let Some(arc) = crate::proc::current_process() {
                let mut p = arc.lock();
                p.state = crate::proc::ProcessState::Zombie;
                p.exit_code = a0 as i32;
            }
            crate::proc::schedule();
            unreachable!()
        }
        SYS_WAIT4 => crate::proc::fork::sys_waitpid(a0 as i32, a1, a2 as u32),
        SYS_KILL => 0,
        SYS_GETPID => crate::proc::current_process()
            .map(|p| p.lock().pid as i64)
            .unwrap_or(1),
        SYS_GETPPID => crate::proc::current_process()
            .map(|p| p.lock().ppid as i64)
            .unwrap_or(0),
        SYS_GETTID | SYS_SET_TID_ADDRESS => crate::proc::current_process()
            .map(|p| p.lock().pid as i64)
            .unwrap_or(1),
        SYS_GETUID | SYS_GETGID => 0,
        SYS_MMAP => mm::sys_mmap(a0, a1 as usize, a2 as i32, a3 as i32, a4 as i32, a5 as i64),
        SYS_MUNMAP => mm::sys_munmap(a0, a1 as usize),
        SYS_BRK => mm::sys_brk(a0),
        SYS_UNAME => misc::sys_uname(a0),
        SYS_CLOCK_GETTIME => misc::sys_clock_gettime(a0, a1),
        SYS_SIGACTION | SYS_SIGPROCMASK | SYS_IOCTL => 0, // stubs
        _ => {
            log::warn!("syscall nr={}", nr);
            -ENOSYS
        }
    }
}

pub mod fs {
    use super::errno::*;
    pub fn sys_write(fd: i32, buf: *const u8, count: usize) -> i64 {
        if buf.is_null() || count == 0 {
            return -EFAULT;
        }
        if fd == 1 || fd == 2 {
            let slice = unsafe { core::slice::from_raw_parts(buf, count) };
            if let Ok(s) = core::str::from_utf8(slice) {
                crate::drivers::serial::write_str(s);
            }
            return count as i64;
        }
        -EBADF
    }
    pub fn sys_read(fd: i32, buf: *mut u8, count: usize) -> i64 {
        if buf.is_null() || count == 0 {
            return -EFAULT;
        }
        if fd == 0 {
            match crate::drivers::keyboard::read_char() {
                Some(c) => {
                    unsafe {
                        *buf = c;
                    }
                    1
                }
                None => -EAGAIN,
            }
        } else {
            -EBADF
        }
    }
}

pub mod mm {
    use super::errno::*;
    use crate::mm::pmm::PAGE_SIZE;
    use crate::mm::vmm::VmaFlags;
    pub fn sys_mmap(addr: u64, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> i64 {
        if len == 0 {
            return -EINVAL;
        }
        let arc = match crate::proc::current_process() {
            Some(p) => p,
            None => return -ENOMEM,
        };
        let mut proc = arc.lock();
        let mut vf = VmaFlags::ANONYMOUS;
        if prot & 1 != 0 {
            vf |= VmaFlags::READ;
        }
        if prot & 2 != 0 {
            vf |= VmaFlags::WRITE;
        }
        if prot & 4 != 0 {
            vf |= VmaFlags::EXEC;
        }
        let virt = if addr != 0 && flags & 0x10 != 0 {
            addr
        } else {
            proc.vm.brk
        };
        let size = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        proc.vm.add_vma(virt, virt + size as u64, vf);
        if !(addr != 0 && flags & 0x10 != 0) {
            proc.vm.brk = virt + size as u64;
        }
        virt as i64
    }
    pub fn sys_munmap(addr: u64, len: usize) -> i64 {
        if addr % PAGE_SIZE as u64 != 0 {
            return -EINVAL;
        }
        let arc = match crate::proc::current_process() {
            Some(p) => p,
            None => return -EINVAL,
        };
        let mut proc = arc.lock();
        let end = addr + len as u64;
        proc.vm.remove_vma(addr, end);
        let mut v = addr;
        while v < end {
            proc.address_space.unmap(v);
            v += PAGE_SIZE as u64;
        }
        0
    }
    pub fn sys_brk(nb: u64) -> i64 {
        let arc = match crate::proc::current_process() {
            Some(p) => p,
            None => return -ENOMEM,
        };
        let mut proc = arc.lock();
        if nb == 0 || nb < proc.vm.brk {
            return proc.vm.brk as i64;
        }
        let old = proc.vm.brk;
        proc.vm.add_vma(
            old,
            nb,
            VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS,
        );
        proc.vm.brk = nb;
        nb as i64
    }
}

pub mod misc {
    use super::errno::*;
    use crate::arch::x86_64::limine::phys_to_virt;
    pub fn sys_uname(ptr: u64) -> i64 {
        if ptr == 0 {
            return -EFAULT;
        }
        let arc = match crate::proc::current_process() {
            Some(p) => p,
            None => return -EFAULT,
        };
        let phys = match arc.lock().address_space.translate(ptr) {
            Some(p) => p,
            None => return -EFAULT,
        };
        let buf = unsafe { core::slice::from_raw_parts_mut(phys_to_virt(phys) as *mut u8, 65 * 6) };
        buf.fill(0);
        buf[..5].copy_from_slice(b"MyOS\0");
        buf[65..69].copy_from_slice(b"myos");
        buf[130..135].copy_from_slice(b"0.1.0");
        buf[195..201].copy_from_slice(b"#1 SMP");
        buf[260..266].copy_from_slice(b"x86_64");
        0
    }
    pub fn sys_clock_gettime(id: u64, ptr: u64) -> i64 {
        if ptr == 0 {
            return -EFAULT;
        }
        let arc = match crate::proc::current_process() {
            Some(p) => p,
            None => return -EFAULT,
        };
        let phys = match arc.lock().address_space.translate(ptr) {
            Some(p) => p,
            None => return -EFAULT,
        };
        let ns = crate::arch::x86_64::timer::nanos();
        unsafe {
            let p = phys_to_virt(phys) as *mut u64;
            p.write(ns / 1_000_000_000);
            p.add(1).write(ns % 1_000_000_000);
        }
        0
    }
}

pub fn handle_int80(frame: &mut InterruptFrame) {
    let r = syscall_dispatch(
        frame.rax, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
    );
    frame.rax = r as u64;
}
