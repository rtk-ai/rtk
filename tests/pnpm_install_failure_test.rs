use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn write_fake_pnpm(dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = dir.join("pnpm.cmd");
        fs::write(
            &path,
            "@echo off\r\n\
             if \"%1\"==\"install\" (\r\n\
             echo Scope: all 3 workspace projects\r\n\
             echo ERR_PNPM_FETCH_404 GET https://registry.npmjs.org/@missing/pkg: Not Found - 404\r\n\
             echo This error happened while installing a direct dependency of /tmp/app\r\n\
             exit /B 1\r\n\
             )\r\n\
             echo unexpected args %* 1>&2\r\n\
             exit /B 2\r\n",
        )
        .expect("write fake pnpm");
        path
    }

    #[cfg(unix)]
    {
        let path = dir.join("pnpm");
        fs::write(
            &path,
            "#!/bin/sh\n\
             if [ \"$1\" = \"install\" ]; then\n\
               printf '%s\\n' 'Scope: all 3 workspace projects'\n\
               printf '%s\\n' 'ERR_PNPM_FETCH_404 GET https://registry.npmjs.org/@missing/pkg: Not Found - 404'\n\
               printf '%s\\n' 'This error happened while installing a direct dependency of /tmp/app'\n\
               exit 1\n\
             fi\n\
             printf '%s\\n' \"unexpected args $*\" >&2\n\
             exit 2\n",
        )
        .expect("write fake pnpm");
        let mut permissions = fs::metadata(&path)
            .expect("fake pnpm metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod fake pnpm");
        path
    }
}

fn path_with_front(front: &Path) -> std::ffi::OsString {
    let current = env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(front.to_path_buf()).chain(env::split_paths(&current));
    env::join_paths(paths).expect("join PATH")
}

#[test]
fn pnpm_install_failure_preserves_stdout_error_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let _fake = write_fake_pnpm(temp.path());

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["pnpm", "install"])
        .env("PATH", path_with_front(temp.path()))
        .output()
        .expect("run rtk");

    assert!(!output.status.success(), "pnpm failure must propagate");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("ERR_PNPM_FETCH_404"),
        "pnpm stdout error must be preserved, got: {combined:?}"
    );
    assert!(
        combined.contains("direct dependency"),
        "pnpm failure context must be preserved, got: {combined:?}"
    );
}
