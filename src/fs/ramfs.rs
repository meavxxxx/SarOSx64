use super::vfs::{alloc_ino, DirEntry, Errno, FileType, Filesystem, Ino, Inode, InodeOps, Stat};
use crate::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub struct RamDir {
    ino: Ino,
    mode: u32,
    children: SpinLock<BTreeMap<String, Arc<Inode>>>,
}

pub struct RamFile {
    ino: Ino,
    mode: u32,
    data: SpinLock<Vec<u8>>,
}

pub struct RamSymlink {
    ino: Ino,
    target: String,
}

fn not_dir<T>() -> Result<T, Errno> {
    Err(Errno::ENOTDIR)
}
fn is_dir<T>() -> Result<T, Errno> {
    Err(Errno::EISDIR)
}
fn no_link<T>() -> Result<T, Errno> {
    Err(Errno::EINVAL)
}

impl RamDir {
    pub fn new_inode(mode: u32) -> Arc<Inode> {
        let ops = Arc::new(RamDir {
            ino: alloc_ino(),
            mode,
            children: SpinLock::new(BTreeMap::new()),
        });
        let ino = ops.ino;
        Inode::new(ino, ops)
    }
}

impl InodeOps for RamDir {
    fn stat(&self) -> Stat {
        Stat {
            ino: self.ino,
            kind: FileType::Directory,
            size: 0,
            mode: self.mode,
            nlink: 2,
            uid: 0,
            gid: 0,
        }
    }
    fn read(&self, _: u64, _: &mut [u8]) -> Result<usize, Errno> {
        is_dir()
    }
    fn write(&self, _: u64, _: &[u8]) -> Result<usize, Errno> {
        is_dir()
    }
    fn truncate(&self, _: u64) -> Result<(), Errno> {
        is_dir()
    }

    fn lookup(&self, name: &str) -> Result<Arc<Inode>, Errno> {
        self.children.lock().get(name).cloned().ok_or(Errno::ENOENT)
    }

    fn readdir(&self, offset: usize) -> Result<Option<DirEntry>, Errno> {
        let ch = self.children.lock();
        Ok(ch.iter().nth(offset).map(|(n, i)| DirEntry {
            name: n.clone(),
            ino: i.ino,
            kind: i.stat().kind,
        }))
    }

    fn create(&self, name: &str, mode: u32) -> Result<Arc<Inode>, Errno> {
        let mut ch = self.children.lock();
        if ch.contains_key(name) {
            return Err(Errno::EEXIST);
        }
        let ops = Arc::new(RamFile {
            ino: alloc_ino(),
            mode,
            data: SpinLock::new(Vec::new()),
        });
        let inode = Inode::new(ops.ino, ops);
        ch.insert(name.to_string(), Arc::clone(&inode));
        Ok(inode)
    }

    fn mkdir(&self, name: &str, mode: u32) -> Result<Arc<Inode>, Errno> {
        let mut ch = self.children.lock();
        if ch.contains_key(name) {
            return Err(Errno::EEXIST);
        }
        let inode = RamDir::new_inode(mode);
        ch.insert(name.to_string(), Arc::clone(&inode));
        Ok(inode)
    }

    fn unlink(&self, name: &str) -> Result<(), Errno> {
        let mut ch = self.children.lock();
        match ch.get(name) {
            None => return Err(Errno::ENOENT),
            Some(i) if i.is_dir() => return Err(Errno::EISDIR),
            _ => {}
        }
        ch.remove(name);
        Ok(())
    }

    fn rmdir(&self, name: &str) -> Result<(), Errno> {
        let mut ch = self.children.lock();
        match ch.get(name) {
            None => return Err(Errno::ENOENT),
            Some(i) if !i.is_dir() => return Err(Errno::ENOTDIR),
            Some(i) if i.ops.readdir(0)?.is_some() => return Err(Errno::ENOTEMPTY),
            _ => {}
        }
        ch.remove(name);
        Ok(())
    }

    fn symlink(&self, name: &str, target: &str) -> Result<Arc<Inode>, Errno> {
        let mut ch = self.children.lock();
        if ch.contains_key(name) {
            return Err(Errno::EEXIST);
        }
        let ops = Arc::new(RamSymlink {
            ino: alloc_ino(),
            target: target.to_string(),
        });
        let inode = Inode::new(ops.ino, ops);
        ch.insert(name.to_string(), Arc::clone(&inode));
        Ok(inode)
    }

    fn readlink(&self) -> Result<String, Errno> {
        no_link()
    }

    fn rename(&self, old: &str, new_dir: &Arc<Inode>, new: &str) -> Result<(), Errno> {
        let inode = self.children.lock().remove(old).ok_or(Errno::ENOENT)?;
        new_dir.ops.insert_child(new, inode)
    }

    fn insert_child(&self, name: &str, child: Arc<Inode>) -> Result<(), Errno> {
        self.children.lock().insert(name.to_string(), child);
        Ok(())
    }
}

impl InodeOps for RamFile {
    fn stat(&self) -> Stat {
        Stat {
            ino: self.ino,
            kind: FileType::Regular,
            size: self.data.lock().len() as u64,
            mode: self.mode,
            nlink: 1,
            uid: 0,
            gid: 0,
        }
    }
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let data = self.data.lock();
        let off = offset as usize;
        if off >= data.len() {
            return Ok(0);
        }
        let n = (data.len() - off).min(buf.len());
        buf[..n].copy_from_slice(&data[off..off + n]);
        Ok(n)
    }
    fn write(&self, offset: u64, buf: &[u8]) -> Result<usize, Errno> {
        let mut data = self.data.lock();
        let off = offset as usize;
        let end = off + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[off..end].copy_from_slice(buf);
        Ok(buf.len())
    }
    fn truncate(&self, size: u64) -> Result<(), Errno> {
        self.data.lock().resize(size as usize, 0);
        Ok(())
    }
    fn lookup(&self, _: &str) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn readdir(&self, _: usize) -> Result<Option<DirEntry>, Errno> {
        not_dir()
    }
    fn create(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn mkdir(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn unlink(&self, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn rmdir(&self, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn symlink(&self, _: &str, _: &str) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn readlink(&self) -> Result<String, Errno> {
        no_link()
    }
    fn rename(&self, _: &str, _: &Arc<Inode>, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn insert_child(&self, _: &str, _: Arc<Inode>) -> Result<(), Errno> {
        not_dir()
    }
}

impl InodeOps for RamSymlink {
    fn stat(&self) -> Stat {
        Stat {
            ino: self.ino,
            kind: FileType::Symlink,
            size: self.target.len() as u64,
            mode: 0o777,
            nlink: 1,
            uid: 0,
            gid: 0,
        }
    }
    fn readlink(&self) -> Result<String, Errno> {
        Ok(self.target.clone())
    }
    fn read(&self, _: u64, _: &mut [u8]) -> Result<usize, Errno> {
        no_link()
    }
    fn write(&self, _: u64, _: &[u8]) -> Result<usize, Errno> {
        no_link()
    }
    fn truncate(&self, _: u64) -> Result<(), Errno> {
        no_link()
    }
    fn lookup(&self, _: &str) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn readdir(&self, _: usize) -> Result<Option<DirEntry>, Errno> {
        not_dir()
    }
    fn create(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn mkdir(&self, _: &str, _: u32) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn unlink(&self, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn rmdir(&self, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn symlink(&self, _: &str, _: &str) -> Result<Arc<Inode>, Errno> {
        not_dir()
    }
    fn rename(&self, _: &str, _: &Arc<Inode>, _: &str) -> Result<(), Errno> {
        not_dir()
    }
    fn insert_child(&self, _: &str, _: Arc<Inode>) -> Result<(), Errno> {
        not_dir()
    }
}

pub struct RamFs {
    root: Arc<Inode>,
}

impl Filesystem for RamFs {
    fn root(&self) -> Arc<Inode> {
        Arc::clone(&self.root)
    }
    fn name(&self) -> &'static str {
        "ramfs"
    }
}

pub fn new_ramfs() -> Arc<dyn Filesystem> {
    Arc::new(RamFs {
        root: RamDir::new_inode(0o755),
    })
}
