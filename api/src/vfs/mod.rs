//! Virtual filesystems

pub mod dev;
mod proc;
mod tmp;

use axerrno::LinuxResult;
use axfs::{FS_CONTEXT, FsContext};
use axfs_ng_vfs::{
    Filesystem,
    path::{Path, PathBuf},
};
pub use starry_core::vfs::{Device, DeviceOps, DirMapping, SimpleFs};
pub use tmp::MemoryFs;

fn mount_at(fs: &FsContext, path: &str, mount_fs: Filesystem) -> LinuxResult<()> {
    if fs.resolve(path).is_err() {
        info!(
            "Skip mounting {} at {} because mount point is missing",
            mount_fs.name(),
            path
        );
        return Ok(());
    }
    fs.resolve(path)?.mount(&mount_fs)?;
    info!("Mounted {} at {}", mount_fs.name(), path);
    Ok(())
}

/// Mount all filesystems
pub fn mount_all() -> LinuxResult<()> {
    let fs = FS_CONTEXT.lock();
    mount_at(&fs, "/dev", dev::new_devfs())?;
    mount_at(&fs, "/dev/shm", tmp::MemoryFs::new())?;
    mount_at(&fs, "/tmp", tmp::MemoryFs::new())?;
    mount_at(&fs, "/proc", proc::new_procfs())?;

    mount_at(&fs, "/sys", tmp::MemoryFs::new())?;
    let mut path = PathBuf::new();
    let mut can_build_sys_path = true;
    for comp in Path::new("/sys/class/graphics/fb0/device").components() {
        if !can_build_sys_path {
            break;
        }
        path.push(comp.as_str());
        if fs.resolve(&path).is_err() {
            can_build_sys_path = false;
        }
    }
    if can_build_sys_path {
        path.push("subsystem");
        let _ = fs.symlink("whatever", &path);
    }
    drop(fs);

    #[cfg(feature = "dev-log")]
    dev::bind_dev_log().expect("Failed to bind /dev/log");

    Ok(())
}
