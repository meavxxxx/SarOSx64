use core::arch::asm;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    _zero: u32,
}

impl IdtEntry {
    pub const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        _zero: 0,
    };

    pub fn interrupt_gate(handler: u64, selector: u16, ist: u8, dpl: u8) -> Self {
        let type_attr = 0x8E | (dpl << 5);
        Self {
            offset_low: (handler & 0xFFFF) as u16,
            selector,
            ist: ist & 0x7,
            type_attr,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: (handler >> 32) as u32,
            _zero: 0,
        }
    }

    pub fn trap_gate(handler: u64, selector: u16, dpl: u8) -> Self {
        let type_attr = 0x8F | (dpl << 5);
        Self {
            offset_low: (handler & 0xFFFF) as u16,
            selector,
            ist: 0,
            type_attr,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: (handler >> 32) as u32,
            _zero: 0,
        }
    }
}

#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

#[repr(C, align(16))]
pub struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    pub const fn new() -> Self {
        Self {
            entries: [IdtEntry::MISSING; 256],
        }
    }

    pub fn set_handler(&mut self, vector: u8, handler: u64, ist: u8) {
        self.entries[vector as usize] =
            IdtEntry::interrupt_gate(handler, crate::arch::x86_64::gdt::SEG_KERNEL_CODE, ist, 0);
    }

    pub fn set_trap(&mut self, vector: u8, handler: u64, dpl: u8) {
        self.entries[vector as usize] =
            IdtEntry::trap_gate(handler, crate::arch::x86_64::gdt::SEG_KERNEL_CODE, dpl);
    }

    pub fn load(&self) {
        let idtr = Idtr {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: self as *const Idt as u64,
        };
        unsafe { asm!("lidt [{0}]", in(reg) &idtr, options(nostack)) };
    }
}

static mut IDT: Idt = Idt::new();

#[derive(Debug)]
#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

macro_rules! isr_stub {
    ($vector:expr, no_err) => {{
        unsafe extern "C" fn stub() {
            core::arch::asm!(
                "push 0",
                concat!("push ", $vector),
                "jmp {common}",
                common = sym isr_common,
                options(noreturn, att_syntax)
            );
        }
        stub as u64
    }};

    ($vector:expr, err) => {{
        unsafe extern "C" fn stun() {
            core::arch::asm!(
                concat!("push ", $vector),
                "jmp {common}",
                common = sym isr_common,
                options(noreturn, att_syntax)
            );
        }
        stub as u64
    }};
}

#[naked]
unsafe extern "C" fn isr_common() {
    core::arch::asm!(
        "push %rax",
        "push %rbx",
        "push %rcx",
        "push %rdx",
        "push %rsi",
        "push %rdi",
        "push %rbp",
        "push %r8",
        "push %r9",
        "push %r10",
        "push %r11",
        "push %r12",
        "push %r13",
        "push %r14",
        "push %r15",

        "mov %rsp, %rdi",
        "and $-16, %rsp",
        "sub $8, %rsp",
        "call {dispatch}",
        "add $8, %rsp",

        "pop %r15",
        "pop %r14",
        "pop %r13",
        "pop %r12",
        "pop %r11",
        "pop %r10",
        "pop %r9",
        "pop %r8",
        "pop %rbp",
        "pop %rdi",
        "pop %rsi",
        "pop %rdx",
        "pop %rcx",
        "pop %rbx",
        "pop %rax",

        "add $16, %rsp",
        "iretq",

        dispatch = sym interrupt_dispatch,
        options(noreturn, att_syntax)
    );
}

#[no_mangle]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    let vector = frame.vector as u8;

    match vector {
        0 => exc_divide_error(frame),
        1 => exc_debug(frame),
        2 => exc_nmi(frame),
        3 => exc_breakpoint(frame),
        4 => exc_overflow(frame),
        5 => exc_bound_range(frame),
        6 => exc_invalid_opcode(frame),
        7 => exc_device_not_available(frame),
        8 => exc_double_fault(frame),
        10 => exc_invalid_tss(frame),
        11 => exc_segment_not_present(frame),
        12 => exc_stack_segment_fault(frame),
        13 => exc_general_protection(frame),
        14 => exc_page_fault(frame),
        16 => exc_x87_fpu(frame),
        17 => exc_alignment_check(frame),
        18 => exc_machine_check(frame),
        19 => exc_simd(frame),

        32..=47 => irq_dispatch(vector - 32, frame),

        0x80 => crate::syscall::handle_int80(frame),

        _ => {
            log::warn!("Spurious interrupt vector={:#x}", vector);
        }
    }
}

use crate::arch::x86_64::pic;

fn irq_dispatch(irq: u8, frame: &mut InterruptFrame) {
    if irq == 7 && pic::is_spurious_irq7() {
        return;
    }
    if irq == 15 && pic::is_spurious_irq15() {
        pic::send_eoi_master();
        return;
    }

    match irq {
        0 => crate::arch::x86_64::timer::irq_time(frame),
        1 => crate::drivers::keyboard::irq_keyboard(frame),
        _ => log::debug!("Unhandled IRQ {}", irq),
    }

    pic::send_eoi(irq);
}

fn exc_divide_error(frame: &InterruptFrame) {
    panic!("#DE Divide Error at RIP={:#018x}", frame.rip);
}

fn exc_debug(frame: &InterruptFrame) {
    log::trace!("#DB Debug exception at RIP={:#018x}", frame.rip);
}

fn exc_nmi(frame: &InterruptFrame) {
    panic!("NMI at RIP={:#018x}", frame.rip);
}

fn exc_breakpoint(frame: &InterruptFrame) {
    log::info!("#BP Breakpoint at RIP={:#018x}", frame.rip);
}

fn exc_overflow(frame: &InterruptFrame) {
    deliver_signal(frame, Signal::SIGSEGV, "Overflow");
}

fn exc_bound_range(frame: &InterruptFrame) {
    deliver_signal(frame, Signal::SIGSEGV, "BOUND Range Exceeded");
}

fn exc_invalid_opcode(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        delive_signal(frame, Signal::SIGILL, "Invalid Opcode");
    } else {
        panic!("#UD Invalid Opcode in kernel at RIP={:#018x}", frame.rip);
    }
}

fn exc_device_not_available(frame: &InterruptFrame) {
    log::warn!("#NM Device Not Available at RIP={:#018x}", frame.rip);
}

fn exc_double_fault(frame: &InterruptFrame) {
    panic!(
        "#DF Double Fault! RSP={:#018x} RIP={:#018x} err={}",
        frame.rsp, frame.rip, frame.error_code
    );
}

fn exc_invalid_tss(frame: &InterruptFrame) {
    panic!(
        "#TS Invalid TSS error={:#x} at RIP={:#018x}",
        frame.error_code, frame.rip
    );
}

fn exc_segment_not_present(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        deliver_signal(frame, Signal::SIGSEGV, "Segment Not Present");
    } else {
        panic!(
            "#NP Segment Not Present error={:#x} at RIP={:#018x}",
            frame.error_code, frame.rip
        );
    }
}

fn exc_stack_segment_fault(frame: &InterruptFrame) {
    panic!(
        "#SS Stack Segment Fault error={:#x} at RIP={:#018x}",
        frame.error_code, frame.rip
    );
}

fn exc_general_protection(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        deliver_signal(frame, Signal::SIGSEGV, "General Protection Fault");
    } else {
        panic!(
            "#GP General Protection Fault error={:#x} at RIP={:#018x} CS={:#x}",
            frame.error_code, frame.rip, frame.cs
        );
    }
}

fn exc_page_fault(frame: &InterruptFrame) {
    let cr2: u64;
    unsafe { asm!("mov %cr2, {}", out(reg) cr2, options(att_syntax)) };

    let present = frame.error_code & 1 != 0;
    let write = frame.error_code & 2 != 0;
    let user = frame.error_code & 4 != 0;
    let reserved = frame.error_code & 8 != 0;
    let instruction = frame.error_code & 16 != 0;

    log::trace!(
        "#PF addr={:#018x} P={} W={} U={} R={} I={} RIP={:#018x}",
        cr2,
        present as u8,
        write as u8,
        user as u8,
        reserved as u8,
        instruction as u8,
        frame.rip
    );

    if reserved {
        panic!("#PF reserved bit violation addr={:#018x}", cr2);
    }

    let handled = crate::mm::vmm::handle_page_fault(cr2, frame.error_code);

    if !handled {
        if user {
            deliver_signal(frame, Signal::SIGSEGV, "Page Fault");
        } else {
            panic!(
                "#PF unhandled in kernel! addr={:#018x} err={:#x} RIP={:#018x}",
                cr2, frame.error_code, frame.rip
            );
        }
    }
}

fn exc_x87_fpu(frame: &InterruptFrame) {
    deliver_signal(frame, Signal::SIGFPE, "x87 FPU Error");
}

fn exc_alignment_check(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        deliver_signal(frame, Signal::SIGBUS, "Alignment Check");
    } else {
        panic!("#AC Alignment Check in kernel at RIP={:#018x}", frame.rip);
    }
}

fn exc_machine_check(frame: &InterruptFrame) {
    panic!("#MC Machine Check Exception at RIP={:#018x}", frame.rip);
}

fn exc_simd(frame: &InterruptFrame) {
    deliver_signal(frame, Signal::SIGFPE, "SIMD Floating-Point Exception");
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Signal {
    SIGSEGV = 11,
    SIGILL = 4,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGTRAP = 5,
}

fn deliver_signal(fram: &InterruptFrame, sig: Signal, reason: &str) {
    log::warn!(
        "Signal {:?} ({}) to current process, RIP={:#018x}",
        sig,
        reason,
        frame.rip
    );
    panic!("Unhandled user signal {:?} ({})", sig, reason);
}

macro_rules! make_stubs {
    (err: $($v:expr),*) => {
        ${
            unsafe extern "C" fn paste::paste!{[<isr_stub_$v>]} () {
                core::arch::asm!(
                    concat!("push ", $v),
                    "jmp {common}",
                    common = sym isr_common,
                    options(noreturn, att_syntax)
                );
            }
        }*
    };
}

pub fn init() {
    unsafe {
        for v in [0u8, 1, 2, 3, 4, 5, 6, 7, 9, 16, 18, 19, 20, 21] {
            IDT.set_handler(v, make_isr_no_err(v as u64), 0);
        }
        for v in [8u8, 10, 11, 12, 13, 14, 17] {
            IDT.set_handler(v, make_isr_err(v as u64), 0);
        }

        IDT.entries[8] = IdtEntry::interrupt_gate(
            make_isr_err(8),
            crate::arch::x86_64::gdt::SEG_KERNEL_CODE,
            1,
            0,
        );

        for irq in 0u8..16 {
            IDT.set_handler(32 + irq, make_isr_no_err(32 + irq as u64), 0);
        }

        IDT.set_trap(0x80, make_isr_no_err(0x80), 3);

        IDT.load();
    }
}

fn make_isr_no_err(vector: u64) -> u64 {
    ISR_NO_ERR_TABLE[vector as usize]
}

fn make_isr_err(vector: u64) -> u64 {
    ISR_ERR_TABLE[vector as usize]
}

static mut ISR_NO_ERR_TABLE: [u64; 256] = [0u64; 256];
static mut ISR_ERR_TABLE: [u64; 256] = [0u64; 256];

pub fn init_tables() {
    unsafe {
        for i in 0..256usize {
            ISR_NO_ERR_TABLE[i] = isr_no_err_stub as u64;
            ISR_ERR_TABLE[i] = isr_err_stub as u64;
        }
    }
}

#[naked]
unsafe extern "C" fn isr_no_err_stub() {
    asm!(
        "push 0",
        "push 0",
        "jmp {c}",
        c = sym isr_common,
        options(noreturn, att_syntax)
    );
}

#[naked]
unsafe extern "C" fn isr_err_stub() {
    asm!(
        "push 0",
        "jmp {c}",
        c = sym isr_common,
        options(noreturn, att_syntax)
    );
}
