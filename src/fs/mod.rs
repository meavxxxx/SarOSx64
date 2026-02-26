pub mod fat32;
pub mod mbr;
pub mod mount;
pub mod path;
pub mod ramfs;
pub mod vfs;

pub use mount::{init, with_vfs, VfsContext};
pub use vfs::{Errno, File, FileType, Inode, Stat};

/// Minimal x86_64 ELF64 static executable: write(1, "Hello!\n", 7) then exit(0).
/// Load address 0x400000, entry 0x400078 (= 64-byte ELF header + 56-byte PT_LOAD phdr).
/// Assembled layout:
///   [0x000] ELF header  (64 bytes)
///   [0x040] PT_LOAD phdr (56 bytes)  — covers entire file from offset 0
///   [0x078] code: write(1,msg,7) then exit(0)  (42 bytes)
///   [0x0a2] "Hello!\n"  (7 bytes)   total = 0xa9 = 169
#[rustfmt::skip]
static HELLO_ELF: &[u8] = &[
    // ── ELF header (64 bytes) ───────────────────────────────────────────────
    0x7f,0x45,0x4c,0x46, 0x02,0x01,0x01,0x00, 0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    //  magic                cls  le   ver  abi    padding (8 bytes)
    0x02,0x00,              // e_type:      ET_EXEC
    0x3e,0x00,              // e_machine:   x86-64
    0x01,0x00,0x00,0x00,   // e_version:   1
    0x78,0x00,0x40,0x00,0x00,0x00,0x00,0x00,  // e_entry:  0x400078
    0x40,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // e_phoff:  64 (right after header)
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // e_shoff:  0 (no sections)
    0x00,0x00,0x00,0x00,   // e_flags:     0
    0x40,0x00,             // e_ehsize:    64
    0x38,0x00,             // e_phentsize: 56
    0x01,0x00,             // e_phnum:     1
    0x40,0x00,             // e_shentsize: 64
    0x00,0x00,             // e_shnum:     0
    0x00,0x00,             // e_shstrndx:  0
    // ── PT_LOAD program header (56 bytes) ───────────────────────────────────
    0x01,0x00,0x00,0x00,   // p_type:    PT_LOAD
    0x05,0x00,0x00,0x00,   // p_flags:   PF_R|PF_X = 5
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // p_offset: 0 (load from file start)
    0x00,0x00,0x40,0x00,0x00,0x00,0x00,0x00,  // p_vaddr:  0x400000
    0x00,0x00,0x40,0x00,0x00,0x00,0x00,0x00,  // p_paddr:  0x400000
    0xa9,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // p_filesz: 169
    0xa9,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // p_memsz:  169
    0x00,0x10,0x00,0x00,0x00,0x00,0x00,0x00,  // p_align:  0x1000
    // ── Code at vaddr 0x400078 (42 bytes) ───────────────────────────────────
    // write(1, msg, 7)  —  syscall #1
    0x48,0xc7,0xc0,0x01,0x00,0x00,0x00,  // mov rax, 1
    0x48,0xc7,0xc7,0x01,0x00,0x00,0x00,  // mov rdi, 1
    // lea rsi, [rip + 21]
    //   rip after this insn = 0x40008d; target = 0x4000a2; disp = 0x15 = 21
    0x48,0x8d,0x35,0x15,0x00,0x00,0x00,  // lea rsi, [rip+21]
    0x48,0xc7,0xc2,0x07,0x00,0x00,0x00,  // mov rdx, 7
    0xcd,0x80,                            // int 0x80
    // exit(0)  — syscall #60
    0x48,0xc7,0xc0,0x3c,0x00,0x00,0x00,  // mov rax, 60
    0x48,0x31,0xff,                        // xor rdi, rdi
    0xcd,0x80,                            // int 0x80
    // ── Data at vaddr 0x4000a2 ──────────────────────────────────────────────
    b'H',b'e',b'l',b'l',b'o',b'!',b'\n',
];

pub fn init_rootfs() {
    let fs = ramfs::new_ramfs();
    init(fs);

    with_vfs(|vfs| {
        let _ = vfs.mkdir("/bin");
        let _ = vfs.mkdir("/etc");
        let _ = vfs.mkdir("/tmp");
        let _ = vfs.mkdir("/home");
        let _ = vfs.mkdir("/home/root");
        let _ = vfs.mkdir("/dev");
        let _ = vfs.mkdir("/proc");
        let _ = vfs.mkdir("/var");
        let _ = vfs.mkdir("/var/log");

        let _ = vfs.write_file("/etc/hostname", b"saros\n");
        let _ = vfs.write_file("/etc/os-release", b"NAME=SarOS\nVERSION=0.1\n");
        let _ = vfs.write_file(
            "/etc/motd",
            b"Welcome to SarOS!\nType 'help' for available commands.\n",
        );

        let _ = vfs.write_file("/bin/hello", HELLO_ELF);

        let _ = vfs.mkdir("/images");
        let _ = vfs.write_file(
            "/images/image.bmp",
            include_bytes!("../drivers/image.bmp"),
        );
    });

    log::info!("VFS: rootfs (ramfs) mounted at /");
}
