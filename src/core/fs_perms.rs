//! Best-effort restrictive permissions for RTK-owned local data.
//!
//! On Windows, RTK relies on per-user local app-data ACLs.

use std::path::Path;

/// Restrict a file RTK writes to owner read/write on Unix.
pub fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    let _ = path;
}

/// Restrict an RTK-owned directory to owner-only access on Unix.
///
/// Callers must only pass RTK's default local-data directories, not user-configured paths.
pub fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }

    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_restrict_file_sets_owner_read_write() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("history.db");
        std::fs::write(&path, "test").unwrap();

        restrict_file(&path);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn test_restrict_dir_sets_owner_only() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("rtk");
        std::fs::create_dir(&path).unwrap();

        restrict_dir(&path);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
