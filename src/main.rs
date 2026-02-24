#![no_std]
#![no_main]
#![feature(
    abi_x86_interrupt,
    naked_functions,
    alloc_error_handler,
    const_mut_refs
)]

extern crate alloc;

mod arch;
mod drivers;
mod mm;
mod proc;
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
    log::info!("Architecture initialized");

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
    println!("SarOSx64 booting...");

    println!("Kernel initialized");
    drivers::vga::set_color(drivers::vga::WHITE, drivers::vga::BLACK);

    arch::x86_64::io::sti();
    log::info!("Interrupts enabled");

    arch::x86_64::timer::calibrate_tsc();

    log::info!("Sar0Sx64 initialization complete");
    println!("All systems initialized. Entering idle loop");

    let demo = proc::Process::new_kernel("demo", demo_task, 10);
    if let Some(p) = demo {
        proc::scheduler::spawn(p);
    }

    loop {
        arch::x86_64::io::hlt();
    }
}

fn demo_task() -> ! {
    log::info!("[demo] Demo kernel thread started");

    let mut count = 0u64;
    loop {
        count += 1;
        if count % 1000 == 0 {
            log::info!(
                "[demo] tick ]={} uptime={}ms free_mem={}K",
                arch::x86_64::timer::ticks(),
                arch::x86_64::timer::uptime_ms(),
                mm::pmm::free_pages() * mm::PAGE_SIZE / 1024,
            );
        }

        proc::scheduler::schedule();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    arch::x86_64::io::cli();

    serial_println!("\n\n=== KERNEL PANIC ===");
    serial_println!("{}", info);

    drivers::vga::set_color(drivers::vga::WHITE, drivers::vga::RED);
    println!("\n *** KERNEL PANIC *** ");

    loop {
        arch::x86_64::io::hlt();
    }
}
