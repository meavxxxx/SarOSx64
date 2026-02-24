use crate::arch::x86_64::gdt::{SEG_KERNEL_CODE, SEG_USER_DATA};
use crate::arch::x86_64::io::{rdmsr, wrmsr, EFER_SCE, MSR_EFER, MSR_LSTAR, MSR_SFMASK, MSR_STAR};

/// Инициализация SYSCALL/SYSRET
pub fn init_syscall() {
    unsafe {
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | EFER_SCE);

        let star =
            ((SEG_KERNEL_CODE as u64) << 32) | ((SEG_USER_DATA as u64 & !3).wrapping_sub(8) << 48);
        wrmsr(MSR_STAR, star);
        wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);
        wrmsr(MSR_SFMASK, 0x0000_0000_0004_0700);
    }

    log::info!("SYSCALL/SYSRET initialized");
}

#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
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

        handler = sym syscall_dispatch_entry,
        options(att_syntax)
    );
}

extern "C" fn syscall_dispatch_entry(
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> i64 {
    crate::syscall::syscall_dispatch(nr, a0, a1, a2, a3, a4, a5)
}

pub fn handle_int80(frame: &mut crate::arch::x86_64::idt::InterruptFrame) {
    let nr = frame.rax;
    let a0 = frame.rdi;
    let a1 = frame.rsi;
    let a2 = frame.rdx;

    let result = crate::syscall::syscall_dispatch(nr, a0, a1, a2, 0, 0, 0);
    frame.rax = result as u64;
}
