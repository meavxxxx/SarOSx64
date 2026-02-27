use crate::arch::x86_64::gdt::{current_tss, SEG_KERNEL_CODE, SEG_USER_CODE, SEG_USER_DATA};
use crate::arch::x86_64::io::{
    rdmsr, wrmsr, EFER_SCE, MSR_EFER, MSR_GS_BASE, MSR_KERNEL_GS, MSR_LSTAR, MSR_SFMASK, MSR_STAR,
};

#[repr(C, align(16))]
struct SyscallCpuLocal {
    _reserved: u64,
    kernel_rsp: u64,
    user_rsp: u64,
}

static mut SYSCALL_CPU_LOCAL: SyscallCpuLocal = SyscallCpuLocal {
    _reserved: 0,
    kernel_rsp: 0,
    user_rsp: 0,
};

pub fn set_kernel_stack(rsp: u64) {
    unsafe {
        SYSCALL_CPU_LOCAL.kernel_rsp = rsp;
    }
}

/// Инициализация SYSCALL/SYSRET
pub fn init_syscall() {
    unsafe {
        // swapgs uses KERNEL_GS on syscall entry from ring 3; keep a small
        // per-cpu area there with kernel stack top at +8 and saved user RSP at +16.
        SYSCALL_CPU_LOCAL.kernel_rsp = current_tss().rsp[0];
        SYSCALL_CPU_LOCAL.user_rsp = 0;

        let gs_base = core::ptr::addr_of!(SYSCALL_CPU_LOCAL) as u64;
        wrmsr(MSR_GS_BASE, 0);
        wrmsr(MSR_KERNEL_GS, gs_base);

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

        // Linux SYSCALL ABI:  rax=nr, rdi=a0, rsi=a1, rdx=a2, r10=a3, r8=a4, r9=a5
        // C calling convention: rdi=1st, rsi=2nd, rdx=3rd, rcx=4th, r8=5th, r9=6th
        // We need nr(rax) in rdi and shift a0..a5 right by one slot.
        // a5(r9) has no register slot → push it as a stack (7th) argument.
        "sub $8, %rsp",             // reserve slot for a5
        "mov %r9, (%rsp)",          // a5 = r9  (7th arg, via stack)
        "mov %r8,  %r9",            // a4 = r8
        "mov %r10, %r8",            // a3 = r10  (Linux uses r10 for arg3 in SYSCALL)
        "mov %rdx, %rcx",           // a2 = rdx
        "mov %rsi, %rdx",           // a1 = rsi
        "mov %rdi, %rsi",           // a0 = rdi
        "mov %rax, %rdi",           // nr = rax  (syscall number)

        "call {handler}",

        "add $8, %rsp",             // discard a5 stack slot

        "pop %r15",
        "pop %r14",
        "pop %r13",
        "pop %r12",
        "pop %rbp",
        "pop %rbx",

        "add $8, %rsp",
        "pop %rcx",
        "pop %r11",

        // Build an IRET frame explicitly and return with iretq instead of sysretq.
        // This is more robust against sysret-specific #GP corner cases.
        "mov %gs:16, %rdx",
        "push ${user_ss}",
        "push %rdx",
        "push %r11",
        "push ${user_cs}",
        "push %rcx",

        "swapgs",
        "iretq",

        handler = sym syscall_dispatch_entry,
        user_cs = const SEG_USER_CODE,
        user_ss = const SEG_USER_DATA,
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
