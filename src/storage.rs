//! Storage: what the phone's data partition holds. The total, used and
//! free space come from `statvfs`; how much of it OSS Phone's own files
//! (the Linux system, apps) and their data take is counted by walking the
//! chroot on a worker thread when the Storage page opens. The rest of the
//! used space is Android's.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// Bytes: the Linux system and apps (/usr, /opt, …), and their data
    /// (/var, /root, /home).
    pub system: u64,
    pub data: u64,
}

/// Directories that hold data rather than the system.
const DATA_DIRS: [&str; 3] = ["var", "root", "home"];
/// Never counted: kernel filesystems and scratch space.
const SKIP_DIRS: [&str; 6] = ["proc", "sys", "dev", "run", "tmp", "mnt"];

pub struct StorageWorker {
    result: Receiver<Breakdown>,
}

impl StorageWorker {
    pub fn spawn() -> Self {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(count(Path::new("/")));
        });
        Self { result }
    }

    pub fn try_recv(&self) -> Option<Breakdown> {
        self.result.try_recv().ok()
    }
}

/// Mount points inside the chroot (other filesystems and bind mounts),
/// which must not be counted as its own files.
fn mount_points() -> HashSet<String> {
    fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4).map(str::to_owned))
        .filter(|point| point != "/")
        .collect()
}

fn count(root: &Path) -> Breakdown {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return Breakdown::default();
    };
    let device = meta.dev();
    let mounts = mount_points();
    let mut breakdown = Breakdown::default();
    let Ok(entries) = fs::read_dir(root) else {
        return breakdown;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let size = walk(&entry.path(), device, &mounts);
        if DATA_DIRS.contains(&name.as_str()) {
            breakdown.data += size;
        } else {
            breakdown.system += size;
        }
    }
    breakdown
}

/// Bytes on disk under `path`, staying on `device` and out of mounts.
fn walk(path: &Path, device: u64, mounts: &HashSet<String>) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.dev() != device {
        return 0;
    }
    let mut total = meta.blocks() * 512;
    if meta.is_dir() {
        if mounts.contains(path.to_string_lossy().as_ref()) {
            return 0;
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                total += walk(&entry.path(), device, mounts);
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_files_under_a_directory() {
        let dir = std::env::temp_dir().join(format!("a26-storage-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/file"), vec![1_u8; 20_000]).unwrap();
        let device = fs::metadata(&dir).unwrap().dev();
        let size = walk(&dir, device, &HashSet::new());
        assert!(size >= 20_000, "{size}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
