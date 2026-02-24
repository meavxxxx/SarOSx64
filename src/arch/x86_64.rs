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
        let mut cr4 = read_cr4();
        cr4 |= CR4_PGE;
        cr4 |= CR4_SMEP;
        cr4 |= CR4_SMAP;
        cr4 |= CR4_FSGSBASE;
        write_cr4(cr4);

        let cr0 = read_cr0();
        write_cr0(cr0 | CR0_WP);

        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | EFER_NXE);
    }

    log::debug!("CPU features: WP, SMEP, SMAP, NXE enabled");
}

pub fn udelay(us: u64) {
    let start = timer::nanos() / 1000;
    while timer::nanos() / 1000 - start < us {
        core::hint::spin_loop();
    }
}
