use core::arch::asm;

#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    asm!("out %al, %dx", in("dx") port, in("al") val,
        options(nomem, nostack, preserves_flags, att_syntax));
}

#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in %dx, %al", in("dx") port, out("al") val,
        options(nomem, nostack, preserves_flags, att_syntax));
    val
}

#[inline(always)]
pub unsafe fn outw(port: u16, val: u16) {
    asm!("out %ax, %dx" in("dx") port, in("ax") val,
        options(nomem, nostack, preserves_flags, att_syntax));
}

#[inline(always)]
pub unsafe fn inw(port: u16) -> {
    let val: u16;
    asm!("in %dx, %ax", in("dx") port, out("ax") val,
        options(nomem, nostack, preserves_flags, att_syntax));
    val
}

#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    asm!("out %eax, %dx", in("dx") port, in("eax") val,
        options(nomem, nostack, preserve_flags, att_synvax));
    val
}

#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    asm!("in %dx, %eax", in("dx") port, out("eax") val,
         options(nomem, nostack, preserves_flags, att_syntax));
    val
}

/// Небольшая задержка через запись в порт 0x80 (POST code port)
#[inline(always)]
pub unsafe fn io_wait() {
    outb(0x80, 0x00);
}

// ─── MSR (Model Specific Registers) ──────────────────────────────────────────

pub const MSR_EFER:        u32 = 0xC000_0080;
pub const MSR_STAR:        u32 = 0xC000_0081;
pub const MSR_LSTAR:       u32 = 0xC000_0082; // SYSCALL target RIP
pub const MSR_CSTAR:       u32 = 0xC000_0083; // SYSCALL compat target
pub const MSR_SFMASK:      u32 = 0xC000_0084; // SYSCALL RFLAGS mask
pub const MSR_FS_BASE:     u32 = 0xC000_0100;
pub const MSR_GS_BASE:     u32 = 0xC000_0101; // текущий GS
pub const MSR_KERNEL_GS:   u32 = 0xC000_0102; // альтернативный GS (swapgs)
pub const MSR_TSC_AUX:     u32 = 0xC000_0103;
pub const MSR_APIC_BASE:   u32 = 0x0000_001B;
pub const MSR_IA32_TSC:    u32 = 0x0000_0010;

#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags)
    );
    ((hi as u64) << 32) | lo as u64
}

#[inline(always)]
pub unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack, preserves_flags)
    );
}

// ─── Control Registers ────────────────────────────────────────────────────────

#[inline(always)]
pub unsafe fn read_cr0() -> u64 {
    let v: u64;
    asm!("mov %cr0, {}", out(reg) v, options(att_syntax, nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn write_cr0(v: u64) {
    asm!("mov {}, %cr0", in(reg) v, options(att_syntax, nomem, nostack));
}

#[inline(always)]
pub unsafe fn read_cr2() -> u64 {
    let v: u64;
    asm!("mov %cr2, {}", out(reg) v, options(att_syntax, nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn read_cr3() -> u64 {
    let v: u64;
    asm!("mov %cr3, {}", out(reg) v, options(att_syntax, nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn write_cr3(v: u64) {
    asm!("mov {}, %cr3", in(reg) v, options(att_syntax, nomem, nostack));
}

#[inline(always)]
pub unsafe fn read_cr4() -> u64 {
    let v: u64;
    asm!("mov %cr4, {}", out(reg) v, options(att_syntax, nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn write_cr4(v: u64) {
    asm!("mov {}, %cr4", in(reg) v, options(att_syntax, nomem, nostack));
}


pub const RFLAGS_IF:    u64 = 1 << 9;
pub const RFLAGS_DF:    u64 = 1 << 10;
pub const RFLAGS_IOPL:  u64 = 3 << 12;
pub const RFLAGS_AC:    u64 = 1 << 18;
pub const RFLAGS_ID:    u64 = 1 << 21;

#[inline(always)]
pub fn read_rflags() -> u64 {
    let v: u64;
    unsafe { asm!("pushfq; pop {}", out(reg) v, options(nomem)) };
    v
}


#[inline(always)]
pub fn sti() {
    unsafe { asm!("sti", options(nomem, nostack)) };
}


#[inline(always)]
pub fn cli() -> u64 {
    let flags = read_rflags();
    unsafe { asm!("cli", options(nomem, nostack)) };
    flags
}


#[inline(always)]
pub fn hlt() {
    unsafe { asm!("hlt", options(nomem, nostack)) };
}


#[inline(always)]
pub unsafe fn invlpg(addr: u64) {
    asm!("invlpg [{0}]", in(reg) addr, options(nostack, preserves_flags));
}


#[inline(always)]
pub unsafe fn flush_tlb_all() {
    let cr3 = read_cr3();
    write_cr3(cr3);
}

// ─── CPUID ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    let eax: u32; let ebx: u32; let ecx: u32; let edx: u32;
    unsafe {
        asm!(
            "cpuid",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("ebx") ebx,
            out("edx") edx,
            options(nomem, nostack, preserves_flags)
        );
    }
    CpuidResult { eax, ebx, ecx, edx }
}


pub fn has_feature_ecx(leaf: u32, bit: u32) -> bool {
    cpuid(leaf, 0).ecx & (1 << bit) != 0
}

pub fn has_feature_edx(leaf: u32, bit: u32) -> bool {
    cpuid(leaf, 0).edx & (1 << bit) != 0
}


pub const CR4_PAE:  u64 = 1 << 5;
pub const CR4_PGE:  u64 = 1 << 7;
pub const CR4_OSFXSR: u64 = 1 << 9;
pub const CR4_SMEP: u64 = 1 << 20;
pub const CR4_SMAP: u64 = 1 << 21;
pub const CR4_FSGSBASE: u64 = 1 << 16;


pub const CR0_WP: u64 = 1 << 16;
pub const CR0_PE: u64 = 1 << 0;
pub const CR0_PG: u64 = 1 << 31;


pub const EFER_SCE:  u64 = 1 << 0;
pub const EFER_LME:  u64 = 1 << 8;
pub const EFER_LMA:  u64 = 1 << 10;
pub const EFER_NXE:  u64 = 1 << 11;
