use super::{shell_print, shell_println};
use crate::fs::mount::with_vfs;
use crate::fs::vfs::FileType;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub fn cmd_help() {
    shell_println!("Built-in commands:");
    shell_println!("  ls [path]          list directory contents");
    shell_println!("  cd <path>          change directory");
    shell_println!("  pwd                print working directory");
    shell_println!("  cat <file>         print file contents");
    shell_println!("  echo <text>        print text");
    shell_println!("  mkdir <path>       create directory");
    shell_println!("  touch <file>       create empty file");
    shell_println!("  rm <file>          remove file");
    shell_println!("  rmdir <dir>        remove empty directory");
    shell_println!("  mv <src> <dst>     move/rename");
    shell_println!("  cp <src> <dst>     copy file");
    shell_println!("  write <file> <text> write text to file");
    shell_println!("  stat <path>        show file info");
    shell_println!("  ln -s <target> <link> create symlink");
    shell_println!("  run <file> [args]  execute ELF binary from VFS");
    shell_println!("  mount [n /path]    mount drive n at /path (no args: list mounts)");
    shell_println!("  umount <path>      unmount filesystem");
    shell_println!("  drives             list detected disk drives");
    shell_println!("  lspci              list PCI devices");
    shell_println!("  view <file.bmp>    display BMP image");
    shell_println!("  clear              clear screen");
    shell_println!("  history            command history");
    shell_println!("  uname              system info");
    shell_println!("  uptime             system uptime");
    shell_println!("  free               memory usage");
    shell_println!("  reboot             restart system");
    shell_println!("  halt               halt system");
}

pub fn cmd_ls(args: &[String]) {
    let path = args.first().map(|s| s.as_str()).unwrap_or(".");

    let entries = with_vfs(|vfs| vfs.readdir_all(path));

    match entries {
        Err(e) => shell_println!("ls: {}: error {}", path, e.0),
        Ok(mut entries) => {
            entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));

            let mut line_len = 0usize;
            let col_width = 20usize;

            for entry in &entries {
                let prefix = match entry.kind {
                    FileType::Directory => "\x1b[34m",
                    FileType::Symlink => "\x1b[36m",
                    FileType::CharDevice => "\x1b[33m",
                    FileType::Regular => "\x1b[0m",
                };
                let suffix = match entry.kind {
                    FileType::Directory => "/",
                    FileType::Symlink => "@",
                    _ => "",
                };

                let display = alloc::format!("{}{}{}\x1b[0m", prefix, entry.name, suffix);
                let raw_len = entry.name.len() + suffix.len();

                shell_print!("{}", display);

                line_len += raw_len;
                if line_len + col_width >= 80 {
                    shell_println!();
                    line_len = 0;
                } else {
                    let pad = col_width.saturating_sub(raw_len);
                    for _ in 0..pad {
                        shell_print!(" ");
                    }
                    line_len += pad;
                }
            }

            if line_len > 0 {
                shell_println!();
            }
        }
    }
}

pub fn cmd_cd(args: &[String]) {
    let path = args.first().map(|s| s.as_str()).unwrap_or("/home/root");
    with_vfs(|vfs| {
        if let Err(e) = vfs.cd(path) {
            shell_println!("cd: {}: error {}", path, e.0);
        }
    });
}

pub fn cmd_pwd() {
    with_vfs(|vfs| {
        shell_println!("{}", vfs.cwd_path);
    });
}

pub fn cmd_cat(args: &[String]) {
    if args.is_empty() {
        shell_println!("cat: missing operand");
        return;
    }
    for path in args {
        match with_vfs(|vfs| vfs.read_file(path)) {
            Ok(data) => match core::str::from_utf8(&data) {
                Ok(s) => {
                    crate::drivers::serial::write_str(s);
                    crate::drivers::vga::write_str(s);
                }
                Err(_) => shell_println!("cat: {}: binary file", path),
            },
            Err(e) => shell_println!("cat: {}: error {}", path, e.0),
        }
    }
}

pub fn cmd_echo(args: &[String]) {
    let s = args.join(" ");
    shell_println!("{}", s);
}

pub fn cmd_mkdir(args: &[String]) {
    if args.is_empty() {
        shell_println!("mkdir: missing operand");
        return;
    }
    for path in args {
        with_vfs(|vfs| {
            if let Err(e) = vfs.mkdir(path) {
                shell_println!("mkdir: {}: error {}", path, e.0);
            }
        });
    }
}

pub fn cmd_touch(args: &[String]) {
    if args.is_empty() {
        shell_println!("touch: missing operand");
        return;
    }
    for path in args {
        with_vfs(|vfs| {
            if let Err(e) = vfs.open(path, crate::fs::vfs::O_CREAT | crate::fs::vfs::O_WRONLY) {
                shell_println!("touch: {}: error {}", path, e.0);
            }
        });
    }
}

pub fn cmd_rm(args: &[String]) {
    let (recursive, paths): (bool, Vec<_>) = {
        let mut r = false;
        let mut p = Vec::new();
        for a in args {
            if a == "-r" || a == "-rf" || a == "-fr" {
                r = true;
            } else {
                p.push(a);
            }
        }
        (r, p)
    };

    for path in paths {
        with_vfs(|vfs| {
            if let Err(e) = vfs.unlink(path) {
                shell_println!("rm: {}: error {}", path, e.0);
            }
        });
    }
}

pub fn cmd_rmdir(args: &[String]) {
    if args.is_empty() {
        shell_println!("rmdir: missing operand");
        return;
    }
    for path in args {
        with_vfs(|vfs| {
            if let Err(e) = vfs.rmdir(path) {
                shell_println!("rmdir: {}: error {}", path, e.0);
            }
        });
    }
}

pub fn cmd_mv(args: &[String]) {
    if args.len() < 2 {
        shell_println!("mv: missing operand");
        return;
    }
    with_vfs(|vfs| {
        if let Err(e) = vfs.rename(&args[0], &args[1]) {
            shell_println!("mv: error {}", e.0);
        }
    });
}

pub fn cmd_cp(args: &[String]) {
    if args.len() < 2 {
        shell_println!("cp: missing operand");
        return;
    }
    let result = with_vfs(|vfs| {
        let data = vfs.read_file(&args[0])?;
        vfs.write_file(&args[1], &data)
    });
    if let Err(e) = result {
        shell_println!("cp: error {}", e.0);
    }
}

pub fn cmd_write(args: &[String]) {
    if args.len() < 2 {
        shell_println!("write: usage: write <file> <text...>");
        return;
    }
    let content = args[1..].join(" ");
    let mut data = content.into_bytes();
    data.push(b'\n');
    with_vfs(|vfs| {
        if let Err(e) = vfs.write_file(&args[0], &data) {
            shell_println!("write: error {}", e.0);
        }
    });
}

pub fn cmd_stat(args: &[String]) {
    if args.is_empty() {
        shell_println!("stat: missing operand");
        return;
    }
    for path in args {
        match with_vfs(|vfs| vfs.stat(path)) {
            Err(e) => shell_println!("stat: {}: error {}", path, e.0),
            Ok(s) => {
                let kind = match s.kind {
                    FileType::Regular => "regular file",
                    FileType::Directory => "directory",
                    FileType::Symlink => "symbolic link",
                    FileType::CharDevice => "character device",
                };
                shell_println!("  File: {}", path);
                shell_println!("  Size: {}  Type: {}", s.size, kind);
                shell_println!(" Inode: {}  Links: {}", s.ino, s.nlink);
                shell_println!("  Mode: {:o}", s.mode);
            }
        }
    }
}

pub fn cmd_ln(args: &[String]) {
    if args.len() < 3 || args[0] != "-s" {
        shell_println!("ln: usage: ln -s <target> <link>");
        return;
    }
    with_vfs(|vfs| {
        if let Err(e) = vfs.symlink(&args[1], &args[2]) {
            shell_println!("ln: error {}", e.0);
        }
    });
}

fn wait_foreground(pid: u32) {
    loop {
        match crate::proc::process_state(pid) {
            None
            | Some(crate::proc::ProcessState::Dead)
            | Some(crate::proc::ProcessState::Zombie) => break,
            _ => crate::proc::scheduler::schedule(),
        }
    }
}

fn resolve_external_path(cmd: &str) -> Option<String> {
    with_vfs(|vfs| {
        if cmd.contains('/') {
            if matches!(vfs.stat(cmd), Ok(stat) if stat.kind == FileType::Regular) {
                return Some(cmd.to_string());
            }
            return None;
        }

        let cwd_candidate = if vfs.cwd_path == "/" {
            alloc::format!("/{}", cmd)
        } else {
            alloc::format!("{}/{}", vfs.cwd_path, cmd)
        };
        if matches!(vfs.stat(&cwd_candidate), Ok(stat) if stat.kind == FileType::Regular) {
            return Some(cwd_candidate);
        }

        let bin_candidate = alloc::format!("/bin/{}", cmd);
        if matches!(vfs.stat(&bin_candidate), Ok(stat) if stat.kind == FileType::Regular) {
            return Some(bin_candidate);
        }

        None
    })
}

pub fn try_run_external(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }

    let path = match resolve_external_path(args[0].as_str()) {
        Some(p) => p,
        None => return false,
    };

    let mut run_args = Vec::with_capacity(args.len());
    run_args.push(path);
    for arg in &args[1..] {
        run_args.push(arg.clone());
    }

    cmd_run(&run_args);
    true
}

pub fn cmd_run(args: &[String]) {
    if args.is_empty() {
        shell_println!("run: usage: run <path> [args...]");
        return;
    }
    let path = &args[0];

    let elf_data = match with_vfs(|vfs| vfs.read_file(path)) {
        Ok(d) => d,
        Err(e) => {
            shell_println!("run: {}: error {}", path, e.0);
            return;
        }
    };

    if !crate::proc::elf::is_valid_elf(&elf_data) {
        shell_println!("run: {}: not a valid ELF64 binary", path);
        return;
    }

    let argv: alloc::vec::Vec<alloc::vec::Vec<u8>> = args
        .iter()
        .map(|s| {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        })
        .collect();

    let envp: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec![
        b"PATH=/bin\0".to_vec(),
        b"HOME=/root\0".to_vec(),
        b"TERM=linux\0".to_vec(),
    ];

    match crate::proc::Process::new_user(path, &elf_data, &argv, &envp, 5) {
        Ok(proc) => {
            let pid = proc.lock().pid;
            crate::proc::spawn(proc);
            shell_println!("Spawned '{}' as pid {}", path, pid);
            wait_foreground(pid);
        }
        Err(e) => shell_println!("run: {}: {}", path, e),
    }
}

pub fn cmd_mount(args: &[String]) {
    if args.len() < 2 {
        // List mounts
        with_vfs(|vfs| {
            for mp in vfs.list_mounts() {
                shell_println!("  {}", mp);
            }
        });
        return;
    }

    // mount <drive_idx> <mountpoint>
    let drive_idx: usize = match args[0].parse() {
        Ok(n) => n,
        Err(_) => { shell_println!("mount: invalid drive index"); return; }
    };
    let mountpoint = args[1].as_str();

    match crate::fs::fat32::probe_drive(drive_idx) {
        Some(fs) => {
            with_vfs(|vfs| {
                if let Err(e) = vfs.mount(mountpoint, fs) {
                    shell_println!("mount: error {}", e.0);
                } else {
                    shell_println!("mounted drive {} at {}", drive_idx, mountpoint);
                }
            });
        }
        None => shell_println!("mount: no supported filesystem found on drive {}", drive_idx),
    }
}

pub fn cmd_umount(args: &[String]) {
    if args.is_empty() {
        shell_println!("umount: usage: umount <mountpoint>");
        return;
    }
    with_vfs(|vfs| {
        if let Err(e) = vfs.umount(&args[0]) {
            shell_println!("umount: {}: error {}", args[0], e.0);
        }
    });
}

pub fn cmd_drives() {
    let count = crate::drivers::ide::drive_count();
    if count == 0 {
        shell_println!("No drives detected.");
        return;
    }
    for i in 0..count {
        if let Some(d) = crate::drivers::ide::drive_info(i) {
            shell_println!(
                "  hd{} — {} [{} MiB, LBA{}]",
                (b'a' + i as u8) as char,
                d.model,
                d.size_mb(),
                if d.lba48 { 48 } else { 28 },
            );
        }
    }
}

pub fn cmd_lspci() {
    crate::drivers::pci::devices(|d| {
        shell_println!(
            "{:02x}:{:02x}.{} [{:04x}:{:04x}] {}",
            d.bus, d.dev, d.func,
            d.vendor_id, d.device_id,
            d.class_name(),
        );
    });
}

pub fn cmd_view(args: &[String]) {
    if args.is_empty() {
        shell_println!("view: usage: view <file.bmp>");
        return;
    }
    let path = args[0].as_str();
    let data = match with_vfs(|vfs| vfs.read_file(path)) {
        Ok(d) => d,
        Err(e) => {
            shell_println!("view: {}: error {}", path, e.0);
            return;
        }
    };
    match crate::drivers::bmp::decode(&data) {
        Some(bmp) => {
            shell_println!(
                "Displaying {}x{} — press any key to exit",
                bmp.width,
                bmp.height
            );
            crate::drivers::vga::draw_bitmap(&bmp);
            crate::drivers::keyboard::wait_key();
            crate::drivers::vga::clear();
        }
        None => {
            shell_println!("view: {}: unsupported format (24-bit uncompressed BMP only)", path);
        }
    }
}

pub fn cmd_clear() {
    crate::drivers::vga::clear();
    crate::drivers::serial::write_str("\x1b[2J\x1b[H");
}
