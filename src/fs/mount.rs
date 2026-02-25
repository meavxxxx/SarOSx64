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
    /// Mount table: (absolute_mount_point, fs_root_inode)
    /// Sorted longest-first for correct prefix matching.
    mounts: Vec<(String, Arc<Inode>)>,
}

impl VfsContext {
    pub fn new(root: Arc<Inode>) -> Self {
        let cwd = Arc::clone(&root);
        Self {
            root,
            cwd,
            cwd_path: "/".to_string(),
            mounts: Vec::new(),
        }
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    fn make_absolute(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else if self.cwd_path == "/" {
            alloc::format!("/{}", path)
        } else {
            alloc::format!("{}/{}", self.cwd_path, path)
        }
    }

    /// Resolve an *absolute* path, checking the mount table first.
    fn resolve_abs(&self, abs: &str) -> Result<Arc<Inode>, Errno> {
        for (mp, fs_root) in &self.mounts {
            if abs == mp.as_str() {
                return Ok(Arc::clone(fs_root));
            }
            // path is inside this mount: e.g. mp="/mnt/d" abs="/mnt/d/foo"
            let prefix = alloc::format!("{}/", mp);
            if abs.starts_with(prefix.as_str()) {
                let rel = &abs[mp.len()..]; // e.g. "/foo"
                return path::resolve(fs_root, fs_root, rel);
            }
        }
        path::resolve(&self.root, &self.cwd, abs)
    }

    pub fn resolve(&self, path: &str) -> Result<Arc<Inode>, Errno> {
        let abs = self.make_absolute(path);
        self.resolve_abs(&abs)
    }

    // ── Mount ─────────────────────────────────────────────────────────────────

    /// Mount `fs` at `mountpoint` (absolute path).
    /// Creates the directory in ramfs if it doesn't exist yet.
    pub fn mount(&mut self, mountpoint: &str, fs: Arc<dyn Filesystem>) -> Result<(), Errno> {
        let mp = if mountpoint.ends_with('/') && mountpoint != "/" {
            mountpoint.trim_end_matches('/').to_string()
        } else {
            mountpoint.to_string()
        };

        // Ensure mount point directory exists in ramfs
        self.mkdir_p(&mp).ok();

        let fs_root = fs.root();

        // Insert sorted by length descending (longest prefix matches first)
        let pos = self.mounts
            .iter()
            .position(|(existing, _)| existing.len() < mp.len())
            .unwrap_or(self.mounts.len());
        self.mounts.insert(pos, (mp.clone(), fs_root));

        log::info!("VFS: mounted {} at {}", fs.name(), mp);
        Ok(())
    }

    pub fn umount(&mut self, mountpoint: &str) -> Result<(), Errno> {
        let before = self.mounts.len();
        self.mounts.retain(|(mp, _)| mp.as_str() != mountpoint);
        if self.mounts.len() == before {
            Err(Errno::ENOENT)
        } else {
            Ok(())
        }
    }

    pub fn list_mounts(&self) -> Vec<String> {
        self.mounts.iter().map(|(mp, _)| mp.clone()).collect()
    }

    // ── VFS operations ────────────────────────────────────────────────────────

    pub fn open(&self, path: &str, flags: u32) -> Result<Arc<File>, Errno> {
        let inode = match self.resolve(path) {
            Ok(i) => {
                if flags & O_CREAT != 0 && flags & O_TRUNC != 0 {
                    i.ops.truncate(0)?;
                }
                i
            }
            Err(Errno::ENOENT) if flags & O_CREAT != 0 => {
                let abs = self.make_absolute(path);
                let (parent, name) = path::resolve_parent(&self.root, &self.cwd, &abs)?;
                parent.ops.create(name, 0o644)?
            }
            Err(e) => return Err(e),
        };
        Ok(File::new(inode, flags))
    }

    pub fn mkdir(&self, path: &str) -> Result<(), Errno> {
        let abs = self.make_absolute(path);
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, &abs)?;
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
        let abs = self.make_absolute(path);
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, &abs)?;
        parent.ops.unlink(name)
    }

    pub fn rmdir(&self, path: &str) -> Result<(), Errno> {
        let abs = self.make_absolute(path);
        let (parent, name) = path::resolve_parent(&self.root, &self.cwd, &abs)?;
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
        let abs = self.make_absolute(path);
        // Normalize: strip trailing slash except root
        self.cwd_path = if abs == "/" {
            abs
        } else {
            abs.trim_end_matches('/').to_string()
        };
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
