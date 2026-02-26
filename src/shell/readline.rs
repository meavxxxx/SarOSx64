use alloc::string::String;
use crate::arch::x86_64::io::{cli, sti, RFLAGS_IF};

fn read_char_blocking() -> u8 {
    loop {
        // Disable interrupts BEFORE checking KB_BUF to close the race window
        // between "buffer empty" check and "set state to Sleeping".
        // If a keyboard IRQ fires in that window while the shell is still
        // Running, wake_up_all_sleeping won't set it Runnable → shell sleeps
        // forever with a char stuck in the buffer.
        let rflags = unsafe { cli() };
        if let Some(c) = crate::drivers::keyboard::read_char() {
            if rflags & RFLAGS_IF != 0 {
                sti();
            }
            return c;
        }
        // Buffer was empty; sleep with IF=0.  context_switch will restore
        // IF=1 (via `or $0x200`) in the next scheduled process, so keyboard
        // IRQs can fire once idle is running, but not in this narrow window.
        crate::proc::scheduler::sleep_current();
        // After waking up the loop restarts and calls cli() again.
    }
}

pub fn readline() -> String {
    let mut line = String::new();

    loop {
        let c = read_char_blocking();

        match c {
            b'\n' | b'\r' => {
                crate::drivers::serial::write_str("\n");
                crate::drivers::vga::write_str("\n");
                return line;
            }
            8 | 127 => {
                if !line.is_empty() {
                    line.pop();
                    crate::drivers::serial::write_str("\x08 \x08");
                    crate::drivers::vga::write_str("\x08");
                }
            }
            3 => {
                crate::drivers::serial::write_str("^C\n");
                crate::drivers::vga::write_str("^C\n");
                return String::new();
            }
            4 if line.is_empty() => {
                return "exit".into();
            }
            c if c >= 0x20 && c < 0x7F => {
                let ch = c as char;
                line.push(ch);
                let s = alloc::format!("{}", ch);
                crate::drivers::serial::write_str(&s);
                crate::drivers::vga::write_str(&s);
            }
            _ => {}
        }
    }
}
