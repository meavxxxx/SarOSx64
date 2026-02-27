mod builtins;
mod readline;

use crate::fs::mount::with_vfs;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub struct Shell {
    pub history: Vec<String>,
}

impl Shell {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    pub fn run(&mut self) -> ! {
        with_vfs(|vfs| {
            if let Ok(motd) = vfs.read_file("/etc/motd") {
                if let Ok(s) = core::str::from_utf8(&motd) {
                    crate::drivers::vga::write_str(s);
                }
            }
        });

        loop {
            let prompt = with_vfs(|vfs| {
                alloc::format!(
                    "\x1b[32mroot@saros\x1b[0m:\x1b[34m{}\x1b[0m# ",
                    vfs.cwd_path
                )
            });
            crate::drivers::serial::write_str(&prompt);
            crate::drivers::vga::write_str(&prompt);

            let line = readline::readline();

            if line.trim().is_empty() {
                continue;
            }

            self.history.push(line.clone());

            let args = parse_args(&line);
            if args.is_empty() {
                continue;
            }

            self.execute(&args);
        }
    }

    fn execute(&mut self, args: &[String]) {
        let cmd = args[0].as_str();
        let rest = &args[1..];

        match cmd {
            "help" => builtins::cmd_help(),
            "ls" => builtins::cmd_ls(rest),
            "cd" => builtins::cmd_cd(rest),
            "pwd" => builtins::cmd_pwd(),
            "cat" => builtins::cmd_cat(rest),
            "echo" => builtins::cmd_echo(rest),
            "mkdir" => builtins::cmd_mkdir(rest),
            "touch" => builtins::cmd_touch(rest),
            "rm" => builtins::cmd_rm(rest),
            "rmdir" => builtins::cmd_rmdir(rest),
            "mv" => builtins::cmd_mv(rest),
            "cp" => builtins::cmd_cp(rest),
            "write" => builtins::cmd_write(rest),
            "stat" => builtins::cmd_stat(rest),
            "ln" => builtins::cmd_ln(rest),
            "run"    => builtins::cmd_run(rest),
            "mount"  => builtins::cmd_mount(rest),
            "umount" => builtins::cmd_umount(rest),
            "drives" => builtins::cmd_drives(),
            "lspci" => builtins::cmd_lspci(),
            "view" => builtins::cmd_view(rest),
            "clear" => builtins::cmd_clear(),
            "history" => {
                for (i, h) in self.history.iter().enumerate() {
                    shell_println!("{:4}  {}", i + 1, h);
                }
            }
            "uname" => shell_println!("SarOS 0.1.0 x86_64"),
            "uptime" => {
                let ms = crate::arch::x86_64::timer::uptime_ms();
                shell_println!("up {}m {}s", ms / 60000, (ms % 60000) / 1000);
            }
            "free" => {
                let free = crate::mm::pmm::free_pages() * crate::mm::PAGE_SIZE / 1024;
                let total = crate::mm::pmm::total_pages() * crate::mm::PAGE_SIZE / 1024;
                shell_println!("              total        free");
                shell_println!("Mem:      {:8} K  {:8} K", total, free);
            }
            "reboot" => unsafe {
                crate::arch::x86_64::io::outb(0x64, 0xFE);
            },
            "halt" | "poweroff" => {
                unsafe {
                    crate::arch::x86_64::io::outw(0x604, 0x2000);
                }
                loop {
                    crate::arch::x86_64::io::hlt();
                }
            }
            _ => {
                if !builtins::try_run_external(args) {
                    shell_println!("{}: command not found", cmd);
                }
            }
        }
    }
}

fn parse_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '"';

    for ch in line.chars() {
        match ch {
            '"' | '\'' if !in_quote => {
                in_quote = true;
                quote_char = ch;
            }
            c if in_quote && c == quote_char => {
                in_quote = false;
            }
            ' ' | '\t' if !in_quote => {
                if !current.is_empty() {
                    args.push(core::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

macro_rules! shell_print {
    ($($a:tt)*) => {{
        let s = alloc::format!($($a)*);
        crate::drivers::serial::write_str(&s);
        crate::drivers::vga::write_str(&s);
    }};
}

macro_rules! shell_println {
    () => { shell_print!("\n") };
    ($($a:tt)*) => { shell_print!("{}\n", alloc::format!($($a)*)) };
}

pub(crate) use shell_print;
pub(crate) use shell_println;

pub fn spawn_shell() -> ! {
    let mut shell = Shell::new();
    shell.run();
}
