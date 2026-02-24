use crate::arch::x86_64::gdt::{SEG_KERNEL_CODE, SEG_KERNEL_DATA, SEG_USER_CODE, SEG_USER_DATA};
use crate::arch::x86_64::io::{rdmsr, wrmsr, EFER_SCE, MSR_EFER, MSR_LSTAR, MSR_SFMASK, MSR_STAR};
use core::arch::asm;

/// Инициализация SYSCALL/SYSRET
pub fn init_syscall() {
    unsafe {
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | EFER_SCE);

        let star =
            ((SEG_KERNEL_CODE as u64) << 32) | ((SEG_USER_DATA as u64 & !3).wrapping_sub(8) << 48);
        wrmsr(MSR_STAR, star);
        wrmsr(MSR_LSTAR, syscall_entry as u64);
        wrmsr(MSR_SFMASK, 0x0000_0000_0004_0700);
    }

    log::info!("SYSCALL/SYSRET initialized");
}

#[naked]
pub unsafe extern "C" fn syscall_entry() {
    asm!(
        "swapgs",

        "mov %rsp, %gs:16",
        "mov %gs:8, %rsp",

        "push %r11",
        "push %rcx",
        "push %rax",

        "push %rbx",
        "push %rbp",
        "push %r12",
        "push %r13",
        "push %r14",
        "push %r15",

        "mov %r10, %rcx",

        "sti",

        "call {handler}",

        "pop %r15",
        "pop %r14",
        "pop %r13",
        "pop %r12",
        "pop %rbp",
        "pop %rbx",

        "add $8, %rsp",
        "pop %rcx",
        "pop %r11",

        "mov %gs:16, %rsp",

        "cli",
        "swapgs",
        "sysretq",

        handler = sym syscall_dispatch,
        options(noreturn, att_syntax)
    );
}

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
    use crate::syscall::nr::*;

    let result = match nr {
        SYS_READ => crate::syscall::fs::sys_read(a0 as i32, a1 as *mut u8, a2 as usize),
        SYS_WRITE => crate::syscall::fs::sys_write(a0 as i32, a1 as *const u8, a2 as usize),
        SYS_OPEN => crate::syscall::fs::sys_open(a0 as *const u8, a1 as i32, a2 as u32),
        SYS_CLOSE => crate::syscall::fs::sys_close(a0 as i32),
        SYS_EXIT => crate::syscall::proc::sys_exit(a0 as i32),
        SYS_FORK => crate::syscall::proc::sys_fork(),
        SYS_GETPID => crate::syscall::proc::sys_getpid(),
        SYS_MMAP => crate::syscall::mm::sys_mmap(
            a0,
            a1 as usize,
            a2 as i32,
            a3 as i32,
            a4 as i32,
            a5 as i64,
        ),
        SYS_MUNMAP => crate::syscall::mm::sys_munmap(a0, a1 as usize),
        SYS_BRK => crate::syscall::mm::sys_brk(a0),
        _ => {
            log::warn!("Unknown syscall nr={}", nr);
            -crate::syscall::errno::ENOSYS as i64
        }
    };

    result
}

pub fn handle_int80(frame: &mut crate::arch::x86_64::idt::InterruptFrame) {
    let nr = frame.rax;
    let a0 = frame.rdi;
    let a1 = frame.rsi;
    let a2 = frame.rdx;

    let result = syscall_dispatch(nr, a0, a1, a2, 0, 0, 0);
    frame.rax = result as u64;
}
