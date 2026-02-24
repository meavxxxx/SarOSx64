use core::fmt::Write;
use log::{Level, LevelFilter, Log, Metadata, Record};

pub struct KernelLogger;

impl Log for KernelLogger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level_str = match record.level() {
            Level::Error => "\x1b[31mERROR\x1b[0m",
            Level::Warn => "\x1b[33m WARN\x1b[0m",
            Level::Info => "\x1b[32m INFO\x1b[0m",
            Level::Debug => "\x1b[34mDEBUG\x1b[0m",
            Level::Trace => "\x1b[90mTRACE\x1b[0m",
        };

        crate::serial_println!("[{}] {}: {}", level_str, record.target(), record.args());

        match record.level() {
            Level::Error | Level::Warn => {
                crate::drivers::vga::set_color(
                    if record.level() == Level::Error {
                        crate::drivers::vga::RED
                    } else {
                        crate::drivers::vga::YELLOW
                    },
                    crate::drivers::vga::BLACK,
                );
            }
            _ => {
                crate::drivers::vga::set_color(
                    crate::drivers::vga::LIGHT_GRAY,
                    crate::drivers::vga::BLACK,
                );
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: KernelLogger = KernelLogger;

pub fn init() {
    log::set_logger(&LOGGER).expect("Logger already set");
    log::set_max_level(LevelFilter::Trace);
}
