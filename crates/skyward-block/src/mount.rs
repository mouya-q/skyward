/// Filesystems Android puts on logical and metadata partitions, in the order
/// a device's fstab lists alternatives for one mount point, with the one
/// the firmware and other radio partitions carry — vfat — which the loader
/// of their images reads them through.
pub const FILESYSTEMS: [&str; 4] = ["erofs", "ext4", "f2fs", "vfat"];

/// The words of a phone's mount options that the kernel takes as flags,
/// as the phone's own fs_mgr reads them, with the kernel's bits for them
/// (linux/mount.h); every other word is the filesystem's own.
const FLAGS: [(&str, u64); 18] = [
    ("noatime", 1 << 10),
    ("noexec", 1 << 3),
    ("nosuid", 1 << 1),
    ("nodev", 1 << 2),
    ("nodiratime", 1 << 11),
    ("ro", 1),
    ("rw", 0),
    ("sync", 1 << 4),
    ("remount", 1 << 5),
    ("bind", 1 << 12),
    ("rec", 1 << 14),
    ("unbindable", 1 << 17),
    ("private", 1 << 18),
    ("slave", 1 << 19),
    ("shared", 1 << 20),
    ("lazytime", 1 << 25),
    ("nosymfollow", 1 << 8),
    ("defaults", 0),
];

/// Split the mount options of a phone's table into the kernel's flags and
/// the options the filesystem reads, joined the way the kernel takes them.
pub fn split_options(options: &[String]) -> (u64, String) {
    let mut flags = 0;
    let mut own = Vec::new();
    for word in options.iter().filter(|w| !w.is_empty()) {
        match FLAGS.iter().find(|(name, _)| name == word) {
            Some((_, bit)) => flags |= bit,
            None => own.push(word.as_str()),
        }
    }
    (flags, own.join(","))
}

/// The mount that shows at a mount point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visible {
    /// (major, minor) of the mounted device.
    pub device: (u32, u32),
    pub filesystem: String,
}

#[cfg(target_os = "android")]
mod sys {
    use super::{Visible, FILESYSTEMS};
    use anyhow::{bail, Context, Result};
    use procfs::process::{all_processes, MMapPath, MountInfo, MountOptFields, Process};
    use procfs::{ProcError, ProcResult};
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::ffi::CString;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::ptr::null;

    /// Where mounts wait while the mount point above them is covered or
    /// uncovered. It is on rootfs, which is private during the moves.
    const PARK: &str = "/skyward/park";

    fn cstr(p: &Path) -> Result<CString> {
        CString::new(p.as_os_str().as_bytes()).with_context(|| format!("{} holds a NUL", p.display()))
    }

    fn os_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    /// Mount `device` read-only at `target` with the first of [`FILESYSTEMS`]
    /// the kernel accepts, and return its name. Only the kernel's answers for
    /// "not this filesystem" move on to the next one.
    pub fn mount_ro(device: &Path, target: &Path) -> Result<&'static str> {
        let (src, dst) = (cstr(device)?, cstr(target)?);
        let mut refused = Vec::new();
        for fs in FILESYSTEMS {
            let ty = CString::new(fs).expect("filesystem names hold no NUL");
            if unsafe { libc::mount(src.as_ptr(), dst.as_ptr(), ty.as_ptr(), libc::MS_RDONLY, null()) } == 0 {
                return Ok(fs);
            }
            let e = os_error();
            match e.raw_os_error() {
                Some(libc::EINVAL | libc::ENODEV) => refused.push(format!("{fs}: {e}")),
                _ => {
                    return Err(e).with_context(|| format!("mount {} on {} as {fs}", device.display(), target.display()))
                }
            }
        }
        bail!("{} holds none of {} ({})", device.display(), FILESYSTEMS.join(", "), refused.join("; "))
    }

    /// Mount `device` at `target` as `filesystem` with `options` as a
    /// phone's own table gives them: writable, unless they say otherwise.
    ///
    /// A filesystem that does not know one of the options refuses the whole
    /// mount with EINVAL rather than the one option, and `inlinecrypt` is
    /// what that happens on: only some of the kernels carry the encryption
    /// it asks for. The key is given to the storage through the mapping laid
    /// over the partition, not through the filesystem's own encryption, so
    /// where the mount is refused that way it is made again without that
    /// one, and nothing of what the partition holds is given up with it.
    pub fn mount(device: &Path, target: &Path, filesystem: &str, options: &[String]) -> Result<()> {
        let refused = |e: &anyhow::Error| e.downcast_ref::<std::io::Error>().and_then(std::io::Error::raw_os_error);
        match once(device, target, filesystem, options) {
            Err(e) if refused(&e) == Some(libc::EINVAL) && options.iter().any(|o| o == "inlinecrypt") => {
                let fewer: Vec<String> = options.iter().filter(|o| o.as_str() != "inlinecrypt").cloned().collect();
                once(device, target, filesystem, &fewer).with_context(|| {
                    format!("mount {} on {} as {filesystem} without inlinecrypt, which the kernel refused", device.display(), target.display())
                })
            }
            other => other,
        }
    }

    fn once(device: &Path, target: &Path, filesystem: &str, options: &[String]) -> Result<()> {
        let (flags, own) = super::split_options(options);
        let (src, dst) = (cstr(device)?, cstr(target)?);
        let ty = CString::new(filesystem).with_context(|| format!("{filesystem} holds a NUL"))?;
        let data = CString::new(own.as_str()).with_context(|| format!("{own} holds a NUL"))?;
        if unsafe { libc::mount(src.as_ptr(), dst.as_ptr(), ty.as_ptr(), flags as libc::c_ulong, data.as_ptr().cast()) } != 0 {
            return Err(os_error())
                .with_context(|| format!("mount {} on {} as {filesystem} with {}", device.display(), target.display(), options.join(",")));
        }
        Ok(())
    }
