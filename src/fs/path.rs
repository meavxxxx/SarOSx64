use super::vfs::{Errno, FileType, Inode, O_CREAT, O_TRUNC};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub fn resolve(root: &Arc<Inode>, cwd: &Arc<Inode>, path: &str) -> Result<Arc<Inode>, Errno> {
    resolve_inner(root, cwd, path, 0)
}

pub fn resolve_parent<'a>(
    root: &Arc<Inode>,
    cwd: &Arc<Inode>,
    path: &'a str,
) -> Result<(Arc<Inode>, &'a str), Errno> {
    let (parent_path, name) = split_last(path);
    let parent = if parent_path.is_empty() {
        Arc::clone(cwd)
    } else {
        resolve(root, cwd, parent_path)?
    };
    Ok((parent, name))
}

fn resolve_inner(
    root: &Arc<Inode>,
    cwd: &Arc<Inode>,
    path: &str,
    depth: u32,
) -> Result<Arc<Inode>, Errno> {
    if depth > 40 {
        return Err(Errno(40));
    }

    let mut current = if path.starts_with('/') {
        Arc::clone(root)
    } else {
        Arc::clone(cwd)
    };

    for component in path.split('/').filter(|s| !s.is_empty()) {
        match component {
            "." => {}
            ".." => {
                current = current
                    .ops
                    .lookup("..")
                    .unwrap_or_else(|_| Arc::clone(root));
            }
            name => {
                if !current.is_dir() {
                    return Err(Errno::ENOTDIR);
                }
                let next = current.ops.lookup(name)?;
                if next.is_symlink() {
                    let target = next.ops.readlink()?;
                    current = resolve_inner(root, &current, &target, depth + 1)?;
                } else {
                    current = next;
                }
            }
        }
    }

    Ok(current)
}

pub fn split_last(path: &str) -> (&str, &str) {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        None => ("", path),
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
    }
}

pub fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}
