use crate::arch::x86_64::idt::InterruptFrame;
use crate::arch::x86_64::io::{inb, outb};
use core::sync::atomic::{AtomicU64, Ordering};

const PIT_CHANNEL0: u16 = 0x40;
const PIT_CHANNEL2: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const PIT_FREQ: u64 = 1_193_182;

pub const TIMER_HZ: u64 = 1000;

static TICK_COUNT: AtomicU64 = AtomicU64::new(0);

static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

pub fn init_pit(hz: u64) {
    let divisor = PIT_FREQ / hz;
    assert!(divisor < 65536, "PIT divisor too large");

    unsafe {
        outb(PIT_CMD, 0b0011_0110);
        outb(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL0, ((divisor >> 8) & 0xFF) as u8);
    }

    log::info!("PIT initialized: {} Hz (divisor={})", hz, divisor);
}

pub fn irq_timer(_frame: &mut InterruptFrame) {
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);

    UPTIME_MS.fetch_add(1000 / TIMER_HZ, Ordering::Relaxed);

    if tick % TIMER_HZ == 0 {
        log::trace!("Uptime: {} s", tick / TIMER_HZ);
    }

    crate::proc::scheduler::tick();
}

pub fn uptime_ms() -> u64 {
    UPTIME_MS.load(Ordering::Relaxed)
}

pub fn ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

pub fn sleep_busy(ms: u64) {
    let end = uptime_ms() + ms;
    while uptime_ms() < end {
        core::hint::spin_loop();
    }
}

#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

static TSC_FREQ_HZ: AtomicU64 = AtomicU64::new(0);

pub fn calibrate_tsc() {
    let ms = 10u64;
    let t0 = rdtsc();
    let start = uptime_ms();

    while uptime_ms() - start < ms {
        core::hint::spin_loop();
    }

    let t1 = rdtsc();
    let elapsed = t1 - t0;
    let freq = elapsed * 1000 / ms;

    TSC_FREQ_HZ.store(freq, Ordering::Relaxed);
    log::info!("TSC frequency: {} MHz", freq / 1_000_000);
}

pub fn tsc_freq_hz() -> u64 {
    TSC_FREQ_HZ.load(Ordering::Relaxed)
}

pub fn nanos() -> u64 {
    let freq = tsc_freq_hz();
    if freq == 0 {
        return uptime_ms() * 1_000_000;
    }
    rdtsc() * 1_000_000_000 / freq
}

pub fn init() {
    init_pit(TIMER_HZ);
}
