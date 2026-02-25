use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

pub type Ino = u64;

static NEXT_INO: AtomicU64 = AtomicU64::new(1);
pub fn alloc_ino() -> Ino {
    NEXT_INO.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
}

#[derive(Debug, Clone, Copy)]
pub struct Stat {
    pub ino: Ino,
    pub kind: FileType,
    pub size: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone)]
pub struct DirEntry {
    pub name: String,
    pub ino: Ino,
    pub kind: FileType,
}

pub trait InodeOps: Send + Sync {
    fn stat(&self) -> Stat;
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Errno>;
    fn write(&self, offset: u64, buf: &[u8]) -> Result<usize, Errno>;
    fn truncate(&self, size: u64) -> Result<(), Errno>;
    fn lookup(&self, name: &str) -> Result<Arc<Inode>, Errno>;
    fn readdir(&self, offset: usize) -> Result<Option<DirEntry>, Errno>;
    fn create(&self, name: &str, mode: u32) -> Result<Arc<Inode>, Errno>;
    fn mkdir(&self, name: &str, mode: u32) -> Result<Arc<Inode>, Errno>;
    fn unlink(&self, name: &str) -> Result<(), Errno>;
    fn rmdir(&self, name: &str) -> Result<(), Errno>;
    fn symlink(&self, name: &str, target: &str) -> Result<Arc<Inode>, Errno>;
    fn readlink(&self) -> Result<String, Errno>;
    fn rename(&self, old_name: &str, new_dir: &Arc<Inode>, new_name: &str) -> Result<(), Errno>;
    fn insert_child(&self, name: &str, child: Arc<Inode>) -> Result<(), Errno>;
}

pub struct Inode {
    pub ino: Ino,
    pub ops: Arc<dyn InodeOps>,
}

impl Inode {
    pub fn new(ino: Ino, ops: Arc<dyn InodeOps>) -> Arc<Self> {
        Arc::new(Self { ino, ops })
    }
    pub fn stat(&self) -> Stat {
        self.ops.stat()
    }
    pub fn is_dir(&self) -> bool {
        self.ops.stat().kind == FileType::Directory
    }
    pub fn is_file(&self) -> bool {
        self.ops.stat().kind == FileType::Regular
    }
    pub fn is_symlink(&self) -> bool {
        self.ops.stat().kind == FileType::Symlink
    }
}

pub struct File {
    pub inode: Arc<Inode>,
    pub offset: crate::sync::spinlock::SpinLock<u64>,
    pub flags: u32,
}

impl File {
    pub fn new(inode: Arc<Inode>, flags: u32) -> Arc<Self> {
        Arc::new(Self {
            inode,
            offset: crate::sync::spinlock::SpinLock::new(0),
            flags,
        })
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut off = self.offset.lock();
        let n = self.inode.ops.read(*off, buf)?;
        *off += n as u64;
        Ok(n)
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let mut off = self.offset.lock();
        if self.flags & O_APPEND != 0 {
            *off = self.inode.stat().size;
        }
        let n = self.inode.ops.write(*off, buf)?;
        *off += n as u64;
        Ok(n)
    }

    pub fn seek_set(&self, pos: u64) {
        *self.offset.lock() = pos;
    }
    pub fn tell(&self) -> u64 {
        *self.offset.lock()
    }

    pub fn readdir_next(&self) -> Result<Option<DirEntry>, Errno> {
        let mut off = self.offset.lock();
        let e = self.inode.ops.readdir(*off as usize)?;
        if e.is_some() {
            *off += 1;
        }
        Ok(e)
    }
}

pub trait Filesystem: Send + Sync {
    fn root(&self) -> Arc<Inode>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub i64);

impl Errno {
    pub const ENOENT: Errno = Errno(2);
    pub const EIO: Errno = Errno(5);
    pub const EBADF: Errno = Errno(9);
    pub const ENOMEM: Errno = Errno(12);
    pub const EACCES: Errno = Errno(13);
    pub const EEXIST: Errno = Errno(17);
    pub const ENOTDIR: Errno = Errno(20);
    pub const EISDIR: Errno = Errno(21);
    pub const EINVAL: Errno = Errno(22);
    pub const ENOSPC: Errno = Errno(28);
    pub const ENOTEMPTY: Errno = Errno(39);
    pub const ENOTSUP: Errno = Errno(95);
    pub fn as_neg_i64(self) -> i64 {
        -self.0
    }
}

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_DIRECTORY: u32 = 0o200000;
