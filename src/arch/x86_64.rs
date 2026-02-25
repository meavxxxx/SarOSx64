pub mod gdt;
pub mod idt;
pub mod io;
pub mod limine;
pub mod pic;
pub mod syscall_entry;
pub mod timer;

use io::*;

pub fn init_bsp(kernel_stack_top: u64) {
    gdt::init_bsp(kernel_stack_top);
    log::debug!("GDT loaded");

    idt::init_tables();
    idt::init();
    log::debug!("IDT loaded");

    pic::init();
    log::debug!("PIC remapped");

    timer::init();
    log::debug!("PIT initialized");
    syscall_entry::init_syscall();
    log::debug!("SYSCALL initialized");

    unsafe {
        // CPUID leaf 7, subleaf 0, EBX: FSGSBASE=0, SMEP=7, SMAP=20
        let cpuid7 = cpuid(7, 0);
        let has_fsgsbase = cpuid7.ebx & (1 << 0) != 0;
        let has_smep    = cpuid7.ebx & (1 << 7) != 0;
        let has_smap    = cpuid7.ebx & (1 << 20) != 0;
        // CPUID 0x80000001, EDX bit 20 = NX
        let has_nxe = cpuid(0x8000_0001, 0).edx & (1 << 20) != 0;

        let mut cr4 = read_cr4();
        cr4 |= CR4_PGE;
        if has_smep    { cr4 |= CR4_SMEP; }
        if has_smap    { cr4 |= CR4_SMAP; }
        if has_fsgsbase { cr4 |= CR4_FSGSBASE; }
        write_cr4(cr4);

        let cr0 = read_cr0();
        write_cr0(cr0 | CR0_WP);

        if has_nxe {
            let efer = rdmsr(MSR_EFER);
            wrmsr(MSR_EFER, efer | EFER_NXE);
        }
    }

    log::debug!("CPU features: WP enabled; SMEP/SMAP/FSGSBASE/NXE if supported");
}

pub fn udelay(us: u64) {
    let start = timer::nanos() / 1000;
    while timer::nanos() / 1000 - start < us {
        core::hint::spin_loop();
    }
}
