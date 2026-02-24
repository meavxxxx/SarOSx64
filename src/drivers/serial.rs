use crate::arch::x86_64::io::{inb, outb};
use crate::sync::spinlock::SpinLock;
use core::fmt;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x01);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
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
