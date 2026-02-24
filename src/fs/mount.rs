use super::path;
use super::vfs::{Errno, File, FileType, Filesystem, Inode, O_CREAT, O_RDWR, O_TRUNC, O_WRONLY};
use crate::sync::spinlock::SpinLock;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub struct VfsContext {
    pub root: Arc<Inode>,
    pub cwd: Arc<Inode>,
    pub cwd_path: String,
}

impl VfsContext {
    pub fn new(root: Arc<Inode>) -> Self {
        let cwd = Arc::clone(&root);
        Self {
            root,
            cwd,
            cwd_path: "/".to_string(),
        }
    }

    pub fn resolve(&self, path: &str) -> Result<Arc<Inode>, Errno> {
        path::resolve(&self.root, &self.cwd, path)
    }

    pub fn open(&self, path: &str, flags: u32) -> Result<Arc<File>, Errno> {
        let inode = match path::resolve(&self.root, &self.cwd, path) {
            Ok(i) => {
                if flags & O_CREAT != 0 {
                    if flags & O_TRUNC != 0 {
                        i.ops.truncate(0)?;
                    }
                }
                i
            }
            Err(Errno::ENOENT) if flags & O_CREAT != 0 => {
                let (parent, name) = path::resolve_parent(&self.root, &self.cwd, path)?;
                parent.ops.create(name, 0o644)?
            }
            Err(e) => return Err(e),
        };

        Ok(File::new(inode, flags))
    }

    pub fn mkdir(&self, path: &str) -> Result<(), Errno> {
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, path)?;
        parent.ops.mkdir(name, 0o755)?;
        Ok(())
    }

    pub fn mkdir_p(&self, path: &str) -> Result<(), Errno> {
        let mut current = if path.starts_with('/') {
            Arc::clone(&self.root)
        } else {
            Arc::clone(&self.cwd)
        };

        for component in path.split('/').filter(|s| !s.is_empty()) {
            match current.ops.lookup(component) {
                Ok(next) => {
                    if !next.is_dir() {
                        return Err(Errno::ENOTDIR);
                    }
                    current = next;
                }
                Err(Errno::ENOENT) => {
                    let next = current.ops.mkdir(component, 0o755)?;
                    current = next;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn unlink(&self, path: &str) -> Result<(), Errno> {
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, path)?;
        parent.ops.unlink(name)
    }

    pub fn rmdir(&self, path: &str) -> Result<(), Errno> {
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, path)?;
        parent.ops.rmdir(name)
    }

    pub fn rename(&self, old: &str, new: &str) -> Result<(), Errno> {
        let (old_parent, old_name) = path::resolve_parent(&self.root, &self.cwd, old)?;
        let (new_parent, new_name) = path::resolve_parent(&self.root, &self.cwd, new)?;
        old_parent.ops.rename(old_name, &new_parent, new_name)
    }

    pub fn symlink(&self, target: &str, link_path: &str) -> Result<(), Errno> {
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, link_path)?;
        parent.ops.symlink(name, target)?;
        Ok(())
    }

    pub fn stat(&self, path: &str) -> Result<super::vfs::Stat, Errno> {
        Ok(self.resolve(path)?.stat())
    }

    pub fn cd(&mut self, path: &str) -> Result<(), Errno> {
        let inode = self.resolve(path)?;
        if !inode.is_dir() {
            return Err(Errno::ENOTDIR);
        }
        self.cwd = inode;
        if path.starts_with('/') {
            self.cwd_path = path.to_string();
        } else if path == ".." {
            if let Some(pos) = self.cwd_path.rfind('/') {
                if pos == 0 {
                    self.cwd_path = "/".to_string();
                } else {
                    self.cwd_path = self.cwd_path[..pos].to_string();
                }
            }
        } else {
            if self.cwd_path == "/" {
                self.cwd_path = alloc::format!("/{}", path);
            } else {
                self.cwd_path = alloc::format!("{}/{}", self.cwd_path, path);
            }
        }
        Ok(())
    }

    pub fn readdir_all(&self, path: &str) -> Result<Vec<super::vfs::DirEntry>, Errno> {
        let inode = self.resolve(path)?;
        if !inode.is_dir() {
            return Err(Errno::ENOTDIR);
        }
        let file = File::new(inode, 0);
        let mut entries = Vec::new();
        while let Some(e) = file.readdir_next()? {
            entries.push(e);
        }
        Ok(entries)
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), Errno> {
        let file = self.open(path, O_WRONLY | O_CREAT | O_TRUNC)?;
        file.write(data)?;
        Ok(())
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, Errno> {
        let file = self.open(path, 0)?;
        let size = file.inode.stat().size as usize;
        let mut buf = alloc::vec![0u8; size];
        let mut total = 0;
        while total < size {
            let n = file.read(&mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }
        buf.truncate(total);
        Ok(buf)
    }
}

static VFS: SpinLock<Option<VfsContext>> = SpinLock::new(None);

pub fn init(root_fs: Arc<dyn Filesystem>) {
    let root = root_fs.root();
    *VFS.lock() = Some(VfsContext::new(root));
}

pub fn with_vfs<F, R>(f: F) -> R
where
    F: FnOnce(&mut VfsContext) -> R,
{
    let mut guard = VFS.lock();
    f(guard.as_mut().expect("VFS not initialized"))
}
