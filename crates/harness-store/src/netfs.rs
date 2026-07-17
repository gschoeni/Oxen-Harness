//! Detect whether a path lives on a network filesystem.
//!
//! SQLite's WAL mode memory-maps a `-shm` file and coordinates through POSIX
//! locks; over NFS/SMB both are unreliable and a mapped page yanked by the
//! server surfaces as SIGBUS. So WAL is only safe on a local filesystem — the
//! store keeps SQLite's default rollback journal when the database directory
//! is on a network mount.
//!
//! Detection is deliberately conservative: only filesystems positively
//! identified as network mounts return `true`, and any probe failure counts
//! as local (the common case, and rollback-journal-on-local is merely slower,
//! never unsafe — the asymmetric risk is WAL-on-network).

use std::path::Path;

/// Whether `path` (or, if it doesn't exist yet, its nearest existing ancestor)
/// is on a network filesystem.
pub fn is_network_filesystem(path: &Path) -> bool {
    // The database file may not exist yet on first open; probe the closest
    // ancestor that does (ultimately `/`, which always exists).
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return false,
        }
    }
    probe_network_fs(probe)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // statfs(2) has no safe wrapper worth a dependency
fn probe_network_fs(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut stat) } != 0 {
        return false;
    }
    // f_fstypename is a fixed NUL-terminated byte array of the fs type name.
    let name = stat
        .f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8 as char)
        .collect::<String>();
    // FUSE mounts ("macfuse"/"osxfuse"/"fusefs", e.g. sshfs) count as network:
    // most in practice are, and the cost of a false positive is only a slower
    // journal mode — versus a SIGBUS if WAL's -shm mmap lands on a real one.
    matches!(
        name.as_str(),
        "nfs" | "smbfs" | "cifs" | "afpfs" | "webdav" | "ftp" | "9p" | "osxfuse" | "macfuse"
    ) || name.starts_with("fuse")
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // statfs(2) has no safe wrapper worth a dependency
#[allow(clippy::unnecessary_cast)] // f_type's width varies by libc target; the cast keeps it portable
fn probe_network_fs(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut stat) } != 0 {
        return false;
    }
    // Magic numbers from statfs(2) for network filesystems.
    const NFS_SUPER_MAGIC: i64 = 0x6969;
    const SMB_SUPER_MAGIC: i64 = 0x517B;
    const SMB2_MAGIC_NUMBER: i64 = 0xFE534D42;
    const CIFS_MAGIC_NUMBER: i64 = 0xFF534D42;
    const CODA_SUPER_MAGIC: i64 = 0x73757245;
    const AFS_SUPER_MAGIC: i64 = 0x5346414F;
    const V9FS_MAGIC: i64 = 0x01021997;
    // FUSE (sshfs and friends): usually network-backed, and a false positive
    // only costs the slower journal mode — versus a SIGBUS if WAL's -shm
    // mmap lands on a real network mount.
    const FUSE_SUPER_MAGIC: i64 = 0x65735546;
    matches!(
        stat.f_type as i64,
        NFS_SUPER_MAGIC
            | SMB_SUPER_MAGIC
            | SMB2_MAGIC_NUMBER
            | CIFS_MAGIC_NUMBER
            | CODA_SUPER_MAGIC
            | AFS_SUPER_MAGIC
            | V9FS_MAGIC
            | FUSE_SUPER_MAGIC
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn probe_network_fs(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_are_not_network_filesystems() {
        // Home and the temp dir are local on every dev/CI machine this runs on.
        assert!(!is_network_filesystem(Path::new("/")));
        assert!(!is_network_filesystem(&std::env::temp_dir()));
    }

    #[test]
    fn nonexistent_paths_probe_the_nearest_existing_ancestor() {
        let missing = std::env::temp_dir().join("does-not-exist/nested/db.sqlite");
        assert!(!is_network_filesystem(&missing));
    }
}
