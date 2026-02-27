use crate::arch::x86_64::io::{inb, outb};
use crate::sync::spinlock::SpinLock;
use core::fmt;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // IER: disable all interrupts while configuring
        outb(COM1 + 3, 0x80); // LCR: DLAB=1 to set baud rate
        outb(COM1 + 0, 0x01); // DLL: divisor low  → 115200 baud
        outb(COM1 + 1, 0x00); // DLH: divisor high
        outb(COM1 + 3, 0x03); // LCR: DLAB=0, 8N1
        outb(COM1 + 2, 0xC7); // FCR: enable FIFO, clear, 14-byte threshold
        outb(COM1 + 4, 0x0B); // MCR: DTR + RTS + OUT2 (OUT2 gates IRQ to PIC)
        outb(COM1 + 1, 0x01); // IER: enable Received Data Available Interrupt
    }
}

/// Called from IRQ4 handler: drain the COM1 FIFO and push bytes to the
/// keyboard buffer so the shell readline loop receives them.
pub fn irq_serial(_frame: &mut crate::arch::x86_64::idt::InterruptFrame) {
    unsafe {
        while inb(COM1 + 5) & 0x01 != 0 {
            let b = inb(COM1);
            crate::drivers::keyboard::push_char(b);
        }
    }
}

fn is_transmit_empty() -> bool {
    unsafe { inb(COM1 + 5) & 0x20 != 0 }
}

pub fn write_byte(b: u8) {
    while !is_transmit_empty() {
        core::hint::spin_loop();
    }
    unsafe {
        outb(COM1, b);
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

struct SerialWriter;
impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

static SERIAL_LOCK: SpinLock<()> = SpinLock::new(());

pub fn print_fmt(args: fmt::Arguments) {
    use fmt::Write;
    let _g = SERIAL_LOCK.lock();
    let _ = SerialWriter.write_fmt(args);
}

#[macro_export]
macro_rules! serial_print {
    ($($a:tt)*) => { $crate::drivers::serial::print_fmt(format_args!($($a)*)) };
}
#[macro_export]
macro_rules! serial_println {
    ()          => { $crate::serial_print!("\n") };
    ($($a:tt)*) => { $crate::serial_print!("{}\n", format_args!($($a)*)) };
}
