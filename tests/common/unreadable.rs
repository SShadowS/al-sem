//! Make a file or folder unreadable for a test (spec §8 recipe).
//!
//! Bad bytes do not work: the `.al` decode is lossy, so a read never fails on
//! content. This helper makes the READ itself fail:
//! - Unix: `chmod 000`, then check that a read now fails (root, or a file
//!   system that ignores modes, still reads: the mode is restored, `None`).
//! - Windows, a file: an open handle with share mode 0, kept in the guard;
//!   every other open fails with a sharing violation.
//! - Windows, a folder: a DENY ACE for list-directory for Everyone
//!   (`icacls <dir> /deny *S-1-1-0:(RD)`), removed on drop.
//!
//! `None` means skip: the caller prints `skipped: cannot make <path>
//! unreadable here` and returns. The guard restores access on drop, so the
//! tempdir can be deleted.

use std::path::{Path, PathBuf};

/// Restores access to `path` when dropped.
pub struct Guard {
    path: PathBuf,
    is_dir: bool,
    #[cfg(unix)]
    mode: u32,
    #[cfg(windows)]
    _handle: Option<std::fs::File>,
}

/// True when a read of `path` (a folder: a listing) still succeeds.
fn still_readable(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        std::fs::read_dir(path).is_ok()
    } else {
        std::fs::read(path).is_ok()
    }
}

#[cfg(unix)]
pub fn unreadable(path: &Path) -> Option<Guard> {
    use std::os::unix::fs::PermissionsExt;
    let is_dir = path.is_dir();
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).ok()?;
    let guard = Guard {
        path: path.to_path_buf(),
        is_dir,
        mode,
    };
    // Dropping the guard restores the mode.
    (!still_readable(path, is_dir)).then_some(guard)
}

#[cfg(windows)]
pub fn unreadable(path: &Path) -> Option<Guard> {
    let is_dir = path.is_dir();
    let guard = if is_dir {
        let denied = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", "*S-1-1-0:(RD)"])
            .output()
            .ok()?
            .status
            .success();
        if !denied {
            return None;
        }
        Guard {
            path: path.to_path_buf(),
            is_dir,
            _handle: None,
        }
    } else {
        use std::os::windows::fs::OpenOptionsExt;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .ok()?;
        Guard {
            path: path.to_path_buf(),
            is_dir,
            _handle: Some(handle),
        }
    };
    // Dropping the guard restores access.
    (!still_readable(path, is_dir)).then_some(guard)
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
        #[cfg(windows)]
        if self.is_dir {
            let _ = std::process::Command::new("icacls")
                .arg(&self.path)
                .args(["/remove:d", "*S-1-1-0"])
                .output();
        }
        // A Windows file guard closes its handle when `_handle` drops.
        let _ = self.is_dir;
    }
}
