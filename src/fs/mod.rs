pub mod mount;
pub mod path;
pub mod ramfs;
pub mod vfs;

pub use mount::{init, with_vfs, VfsContext};
pub use vfs::{Errno, File, FileType, Inode, Stat};

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
    });

    log::info!("VFS: rootfs (ramfs) mounted at /");
}
