#![no_std]
#![no_main]
#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    unused_macros,
    unused_unsafe,
    static_mut_refs,
    function_casts_as_integer
)]
#![feature(abi_x86_interrupt, alloc_error_handler, never_type)]

extern crate alloc;

mod arch;
mod drivers;
mod fs;
mod mm;
mod proc;
mod shell;
mod sync;
mod syscall;

#[link_section = ".limine_reqs"]
#[used]
static _MEMMAP_REQ: &arch::x86_64::limine::MemoryMapRequest = &arch::x86_64::limine::MEMMAP_REQUEST;

#[link_section = ".limine_reqs"]
#[used]
static _HHDM_REQ: &arch::x86_64::limine::HhdmRequest = &arch::x86_64::limine::HHDM_REQUEST;

#[link_section = ".limine_reqs"]
#[used]
static _KADDR_REQ: &arch::x86_64::limine::KernelAddressRequest =
    &arch::x86_64::limine::KERNEL_ADDR_REQUEST;

#[link_section = ".limine_reqs"]
#[used]
static _FB_REQ: &arch::x86_64::limine::FramebufferRequest =
    &arch::x86_64::limine::FRAMEBUFFER_REQUEST;

const KERNEL_STACK_SIZE: usize = 64 * 1024;

#[repr(C, align(16))]
struct KernelStack([u8; KERNEL_STACK_SIZE]);

static KERNEL_STACK: KernelStack = KernelStack([0; KERNEL_STACK_SIZE]);

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    drivers::serial::init();
    serial_println!("=== Kernel booting ===");

    drivers::logger::init();
    log::info!("Logger initialized");

    let kernel_stack_top = unsafe { KERNEL_STACK.0.as_ptr().add(KERNEL_STACK_SIZE) as u64 };
    arch::x86_64::init_bsp(kernel_stack_top);

    mm::pmm::init();
    log::info!(
        "PMM: {} MiB free / {} MiB total",
        mm::pmm::free_pages() * mm::PAGE_SIZE / 1024 / 1024,
        mm::pmm::total_pages() * mm::PAGE_SIZE / 1024 / 1024,
    );

    mm::vmm::init();
    log::info!("VMM initialized");

    drivers::vga::init();
    drivers::vga::set_color(drivers::vga::LIGHT_GREEN, drivers::vga::BLACK);
    println!("SarOS 0.1.0");
    drivers::vga::set_color(drivers::vga::WHITE, drivers::vga::BLACK);

    drivers::pci::init();
    drivers::ide::init();

    fs::init_rootfs();
    log::info!("Filesystem initialized");

    arch::x86_64::io::sti();
    log::info!("Interrupts enabled");

    arch::x86_64::timer::calibrate_tsc();

    let idle = proc::Process::new_kernel("idle", idle_task, u8::MAX);
    if let Some(p) = idle {
        proc::scheduler::spawn(p);
    }

    let sh = proc::Process::new_kernel("shell", shell_task, 5);
    if let Some(p) = sh {
        proc::scheduler::spawn(p);
    }

    proc::scheduler::schedule();

    loop {
        arch::x86_64::io::hlt();
    }
}

fn idle_task() -> ! {
    loop {
        arch::x86_64::io::hlt();
    }
}

fn shell_task() -> ! {
    shell::spawn_shell();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    arch::x86_64::io::cli();

    serial_println!("\n\n=== KERNEL PANIC ===");
    serial_println!("{}", info);

    drivers::vga::set_color(drivers::vga::WHITE, drivers::vga::RED);
    println!("\n *** KERNEL PANIC *** ");
    if let Some(loc) = info.location() {
        println!("{}:{}", loc.file(), loc.line());
    }
    println!("{}", info.message());

    loop {
        arch::x86_64::io::hlt();
    }
}
