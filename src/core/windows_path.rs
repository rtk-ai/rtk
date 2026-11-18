//! Windows PATH candidates may expose metadata without granting execution.
//!
//! `which` checks metadata and PATHEXT, not access rights. In particular, a
//! package-private executable can shadow a usable binary later on PATH.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(super) fn resolve(name: &str) -> which::Result<PathBuf> {
    select(name, which::which_all(name)?)
}

fn select(name: &str, candidates: impl Iterator<Item = PathBuf>) -> which::Result<PathBuf> {
    // An explicit path must never silently select a different executable.
    let explicit = Path::new(name).components().count() > 1;
    let mut first_denied = None;
    for path in candidates {
        if explicit || !access_denied(&path) {
            return Ok(path);
        }
        first_denied.get_or_insert(path);
    }
    // Keep the original access-denied diagnostic when no usable alternative
    // exists. Returning "not found" would also trigger native PATH fallback.
    first_denied.ok_or(which::Error::CannotFindBinaryPath)
}

fn access_denied(path: &Path) -> bool {
    // This is a narrow access probe, not an execution guarantee. Images can
    // execute without FILE_READ_DATA; batch scripts are instead read by cmd.exe
    // and need not have FILE_EXECUTE permission. Do not request GENERIC_READ,
    // READ_CONTROL or SYNCHRONIZE and reject otherwise usable candidates.
    const FILE_READ_DATA: u32 = 0x0001;
    const FILE_EXECUTE: u32 = 0x0020;
    let batch = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    let access = if batch { FILE_READ_DATA } else { FILE_EXECUTE };
    OpenOptions::new()
        .access_mode(access)
        .open(path)
        .is_err_and(|error| error.kind() == ErrorKind::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // Touch only generated files. Keep their original ACL so even a failed
    // assertion restores it before TempDir cleanup. No machine/user ACL changes.
    struct DeniedFile {
        path: PathBuf,
        sddl: String,
    }

    fn powershell(path: &Path, script: &str, sddl: &str) -> std::process::Output {
        let shell = which::which("powershell.exe").expect("Windows PowerShell must be on PATH");
        Command::new(shell)
            // A parent pwsh 7 exports an incompatible module search path for
            // inbox Windows PowerShell. Let it construct its own defaults.
            .env_remove("PSModulePath")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RTK_TEST_ACL_PATH", path)
            .env("RTK_TEST_ACL_SDDL", sddl)
            .output()
            .expect("PowerShell must run on Windows")
    }

    impl DeniedFile {
        fn new(path: &Path, right: &str) -> Self {
            let output = powershell(
                path,
                &format!(
                    "$ErrorActionPreference='Stop'; $p=$env:RTK_TEST_ACL_PATH; $a=Get-Acl -LiteralPath $p; $a.Sddl; $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User; $r=[Security.AccessControl.FileSystemAccessRule]::new($sid,[Security.AccessControl.FileSystemRights]::{right},[Security.AccessControl.AccessControlType]::Deny); $a.AddAccessRule($r); Set-Acl -LiteralPath $p -AclObject $a"
                ),
                "",
            );
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            Self {
                path: path.to_owned(),
                sddl: String::from_utf8(output.stdout).unwrap().trim().to_owned(),
            }
        }
    }

    impl Drop for DeniedFile {
        fn drop(&mut self) {
            let output = powershell(
                &self.path,
                "$ErrorActionPreference='Stop'; $a=[Security.AccessControl.FileSecurity]::new(); $a.SetSecurityDescriptorSddlForm($env:RTK_TEST_ACL_SDDL); Set-Acl -LiteralPath $env:RTK_TEST_ACL_PATH -AclObject $a",
                &self.sddl,
            );
            assert!(
                output.status.success(),
                "failed to restore temporary file ACL: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn search(name: &str, dirs: &[&Path], cwd: &Path) -> which::Result<PathBuf> {
        let paths = std::env::join_paths(dirs).unwrap();
        select(name, which::which_in_all(name, Some(paths), cwd)?)
    }

    #[test]
    fn skips_only_inaccessible_bare_name_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let bad = temp.path().join("restricted directory");
        let good = temp.path().join("usable directory");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::create_dir_all(&good).unwrap();
        let denied = bad.join("rtk-path-probe.exe");
        let usable = good.join("rtk-path-probe.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &denied).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), &usable).unwrap();
        for right in ["ExecuteFile", "ReadAndExecute"] {
            let _acl = DeniedFile::new(&denied, right);
            assert!(std::fs::symlink_metadata(&denied).unwrap().is_file());
            assert_eq!(
                Command::new(&denied)
                    .arg("--list")
                    .output()
                    .unwrap_err()
                    .kind(),
                ErrorKind::PermissionDenied,
                "denial: {right}"
            );
            assert_eq!(
                search("rtk-path-probe", &[&bad, &good], temp.path()).unwrap(),
                usable
            );
            assert_eq!(
                search("rtk-path-probe.exe", &[&bad, &good], temp.path()).unwrap(),
                usable
            );
            assert_eq!(
                search("rtk-path-probe", &[&good, &bad], temp.path()).unwrap(),
                usable
            );
            // No alternate target, and explicit paths, retain their original error.
            assert_eq!(
                search("rtk-path-probe", &[&bad], temp.path()).unwrap(),
                denied
            );
            assert_eq!(
                search(denied.to_str().unwrap(), &[&good], temp.path()).unwrap(),
                denied
            );
            let relative = Path::new("restricted directory").join("rtk-path-probe.exe");
            assert_eq!(
                search(relative.to_str().unwrap(), &[&good], temp.path()).unwrap(),
                denied
            );
        }
        // Read and execute permissions are distinct for native images. A
        // loadable image must not be skipped just because ordinary reads fail.
        {
            let _acl = DeniedFile::new(&denied, "ReadData");
            assert_eq!(
                std::fs::File::open(&denied).unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
            assert!(
                Command::new(&denied)
                    .arg("--list")
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            assert_eq!(
                search("rtk-path-probe", &[&bad, &good], temp.path()).unwrap(),
                denied
            );
        }
        // Removing the denial restores ordinary first-match precedence.
        assert_eq!(
            search("rtk-path-probe", &[&bad, &good], temp.path()).unwrap(),
            denied
        );
        assert!(search("rtk-missing-path-probe", &[&bad, &good], temp.path()).is_err());
    }

    #[test]
    fn keeps_pathext_precedence_and_unknown_probe_errors() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("rtk-order.cmd"), "@echo off\r\n").unwrap();
        std::fs::copy(
            std::env::current_exe().unwrap(),
            temp.path().join("rtk-order.exe"),
        )
        .unwrap();
        let paths = std::env::join_paths([temp.path()]).unwrap();
        let original = which::which_in("rtk-order", Some(&paths), temp.path()).unwrap();
        assert_eq!(
            search("rtk-order", &[temp.path()], temp.path()).unwrap(),
            original
        );
        // A sharing violation, disappearance, or other unknown probe failure
        // is not permission to silently change the executable.
        let gone = temp.path().join("disappeared.exe");
        assert_eq!(
            select("disappeared", [gone.clone(), original].into_iter()).unwrap(),
            gone
        );
    }

    #[test]
    fn batch_scripts_need_read_access_not_execute_access() {
        let temp = tempfile::tempdir().unwrap();
        for ext in ["cmd", "bat"] {
            let path = temp.path().join(format!("rtk-script.{ext}"));
            std::fs::write(&path, "@echo off\r\necho script-ok\r\n").unwrap();
            {
                let _acl = DeniedFile::new(&path, "ExecuteFile");
                assert!(!access_denied(&path));
                let output = Command::new(&path).output().unwrap();
                assert!(output.status.success());
                assert!(String::from_utf8_lossy(&output.stdout).contains("script-ok"));
                assert_eq!(
                    search("rtk-script", &[temp.path()], temp.path()).unwrap(),
                    path
                );
            }
            {
                let _acl = DeniedFile::new(&path, "ReadData");
                assert!(access_denied(&path));
            }
            std::fs::remove_file(path).unwrap();
        }
    }
}
