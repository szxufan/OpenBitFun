//! Writers cache the reference index. Cleanup can update the local writer
//! under its service lock, but must wait until any other writer has closed.
use super::{entries, invalid, SnapshotResult};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// A view in another process may outlive a cleanup. Capture the version
/// before loading its index. Views opened during removal are never cached.
pub(in crate::service::snapshot) struct ViewStamp {
    path: PathBuf,
    version: Option<[u8; 16]>,
}

impl ViewStamp {
    pub(in crate::service::snapshot) fn capture(locks: &Path) -> Self {
        let path = locks.join("snapshot-writers/maintenance.lock");
        let version = Self::read(&path);
        Self { path, version }
    }

    pub(in crate::service::snapshot) fn current(&self) -> bool {
        self.version.is_some() && self.version == Self::read(&self.path)
    }

    fn read(path: &Path) -> Option<[u8; 16]> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some([0; 16]),
            Err(_) => return None,
        };
        file.try_lock_shared().ok()?;
        let mut version = [0; 16];
        if file.metadata().ok()?.len() > 0 {
            file.read_exact(&mut version).ok()?;
        }
        Some(version)
    }
}

fn gate(locks: &Path) -> SnapshotResult<(File, PathBuf)> {
    let dir = locks.join("snapshot-writers");
    fs::create_dir_all(&dir)?;
    super::checked_path(locks, &dir.join("maintenance.lock"))?;
    let gate = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("maintenance.lock"))?;
    Ok((gate, dir))
}

pub(crate) struct WriterLease {
    _file: File,
    path: PathBuf,
}

impl WriterLease {
    pub(in crate::service::snapshot) fn register(locks: &Path) -> SnapshotResult<Self> {
        let (gate, dir) = gate(locks)?;
        gate.lock_shared()?;
        let path = dir.join(format!("{}.writer", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        file.try_lock().map_err(invalid)?;
        Ok(Self { _file: file, path })
    }
}

impl Drop for WriterLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn exclusive(locks: &Path, own: Option<&WriterLease>) -> SnapshotResult<File> {
    let (mut gate, dir) = gate(locks)?;
    gate.try_lock().map_err(invalid)?;
    for entry in entries(&dir)? {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "writer") || own.is_some_and(|w| w.path == path) {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(invalid("Invalid snapshot writer lease"));
        }
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        file.try_lock()
            .map_err(|_| invalid("Snapshot store is in use by another writer"))?;
        drop(file);
        fs::remove_file(path)?;
    }
    gate.write_all(uuid::Uuid::new_v4().as_bytes())?;
    Ok(gate)
}
