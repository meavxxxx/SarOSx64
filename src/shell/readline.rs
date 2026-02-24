use alloc::string::String;

pub fn readline() -> String {
    let mut line = String::new();

    loop {
        let c = crate::drivers::keyboard::read_char_blocking();

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
