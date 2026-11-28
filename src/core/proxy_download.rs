//! Fetches the `rtk-proxy` daemon of this rtk's version into rtk's data folder, for `rtk init -g`
//! when the user sets the daemon up and it is not installed, or when the copy rtk fetched earlier
//! no longer matches rtk. The archive comes from the GitHub release of rtk's own version; it must
//! match that release's `checksums.txt` and report that version before it replaces anything.

use crate::core::{deps, http, user_dirs};
use crate::llm_proxy::PROXY_BIN;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const RELEASES: &str = "https://github.com/rtk-ai/rtk/releases/download";
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
const MAX_CHECKSUMS: u64 = 1024 * 1024;
const LICENSES: [&str; 2] = ["LICENSE", "LICENSE-ENGINE"];

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn asset() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("rtk-proxy-aarch64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("rtk-proxy-x86_64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("rtk-proxy-aarch64-unknown-linux-musl.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("rtk-proxy-x86_64-unknown-linux-musl.tar.gz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("rtk-proxy-x86_64-pc-windows-msvc.zip")
    } else {
        None
    }
}

pub fn managed_binary() -> Option<PathBuf> {
    user_dirs::data().map(|dir| dir.join("bin").join(PROXY_BIN))
}

pub fn is_managed(bin: &Path) -> bool {
    managed_binary().is_some_and(|managed| managed == bin)
}

pub fn reported_version(bin: &Path) -> Option<String> {
    // nosemgrep: dynamic-command-execution — the rtk-proxy rtk's own locator found, or the one it just verified against checksums.txt, run for --version only
    let out = Command::new(bin).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    if words.next()? != "rtk-proxy" {
        return None;
    }
    words.next().map(str::to_string)
}

pub fn install() -> Result<PathBuf> {
    let asset = asset().context("no rtk-proxy is published for this computer")?;
    let dest = managed_binary().context("could not resolve rtk's data folder")?;
    install_from(RELEASES, version(), asset, &dest)?;
    Ok(dest)
}

pub fn remove(dry_run: bool) -> Result<Vec<PathBuf>> {
    let Some(bin) = managed_binary() else {
        return Ok(Vec::new());
    };
    let Some(dir) = bin.parent() else {
        return Ok(Vec::new());
    };
    let present: Vec<PathBuf> = std::iter::once(bin.clone())
        .chain(LICENSES.iter().map(|name| license_path(dir, name)))
        .filter(|file| file.is_file())
        .collect();
    if !dry_run {
        for file in &present {
            std::fs::remove_file(file).with_context(|| format!("remove {}", file.display()))?;
        }
        let _ = std::fs::remove_dir(dir);
    }
    Ok(present)
}

fn license_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("rtk-proxy-{name}"))
}

fn install_from(base: &str, version: &str, asset: &str, dest: &Path) -> Result<()> {
    let release = format!("{base}/v{version}");
    let sums_url = format!("{release}/checksums.txt");
    let mut response = http::get_release_asset(&sums_url)
        .with_context(|| format!("no rtk-proxy release found for rtk {version}"))?;
    let sums = http::read_text_lossy(&mut response, MAX_CHECKSUMS)
        .with_context(|| format!("read {sums_url}"))?;
    let expected =
        expected_sha256(&sums, asset).with_context(|| format!("{sums_url} lists no {asset}"))?;
    let dir = dest
        .parent()
        .context("the rtk-proxy destination has no folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let work = dir.join(format!(".rtk-proxy-download-{}", std::process::id()));
    if work.exists() {
        std::fs::remove_dir_all(&work).with_context(|| format!("clear {}", work.display()))?;
    }
    std::fs::create_dir(&work).with_context(|| format!("create {}", work.display()))?;
    let outcome = unpack_verified(&release, version, asset, &expected, &work, dest);
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

fn unpack_verified(
    release: &str,
    version: &str,
    asset: &str,
    expected: &str,
    work: &Path,
    dest: &Path,
) -> Result<()> {
    let url = format!("{release}/{asset}");
    let archive = work.join(asset);
    let actual = deps::save_response(http::get_release_asset(&url)?, &url, &archive, MAX_ARCHIVE)?;
    if !actual.eq_ignore_ascii_case(expected) {
        anyhow::bail!(
            "{asset} does not match the checksum published with rtk {version}; nothing was installed"
        );
    }
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(work)
        .status()
        .context("run tar to unpack rtk-proxy")?;
    if !status.success() {
        anyhow::bail!("could not unpack {asset}");
    }
    let unpacked = work.join(PROXY_BIN);
    if !unpacked.is_file() {
        anyhow::bail!("{asset} holds no {PROXY_BIN}");
    }
    deps::make_executable(&unpacked)?;
    match reported_version(&unpacked) {
        Some(found) if found == version => {}
        Some(found) => anyhow::bail!(
            "the downloaded rtk-proxy reports version {found}, not {version}; nothing was installed"
        ),
        None => anyhow::bail!(
            "the downloaded rtk-proxy does not run on this computer; nothing was installed"
        ),
    }
    let dir = dest
        .parent()
        .context("the rtk-proxy destination has no folder")?;
    for name in LICENSES {
        let from = work.join(name);
        if from.is_file() {
            let to = license_path(dir, name);
            std::fs::rename(&from, &to).with_context(|| format!("install {}", to.display()))?;
        }
    }
    std::fs::rename(&unpacked, dest).with_context(|| format!("install {}", dest.display()))
}

fn expected_sha256(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_name_the_asset_they_hash() {
        let hash = "a".repeat(64);
        let other = "b".repeat(64);
        let sums = format!(
            "{other}  rtk-aarch64-apple-darwin.tar.gz\n{hash}  rtk-proxy-aarch64-apple-darwin.tar.gz\n{other} *rtk-proxy-x86_64-pc-windows-msvc.zip\nnot-a-hash  rtk-proxy-x86_64-apple-darwin.tar.gz\n"
        );
        assert_eq!(
            expected_sha256(&sums, "rtk-proxy-aarch64-apple-darwin.tar.gz"),
            Some(hash)
        );
        assert_eq!(
            expected_sha256(&sums, "rtk-proxy-x86_64-pc-windows-msvc.zip"),
            Some(other)
        );
        assert_eq!(
            expected_sha256(&sums, "rtk-proxy-x86_64-apple-darwin.tar.gz"),
            None
        );
        assert_eq!(expected_sha256(&sums, "rtk-proxy-riscv.tar.gz"), None);
    }

    #[cfg(unix)]
    mod served {
        use super::super::*;
        use sha2::{Digest, Sha256};
        use std::collections::HashMap;
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        const ASSET: &str = "rtk-proxy-test.tar.gz";

        fn archive(root: &Path, reports: &str) -> Vec<u8> {
            let src = root.join("src");
            std::fs::create_dir_all(&src).expect("create archive source");
            let bin = src.join(PROXY_BIN);
            std::fs::write(&bin, format!("#!/bin/sh\necho \"rtk-proxy {reports}\"\n"))
                .expect("write stub daemon");
            deps::make_executable(&bin).expect("make stub executable");
            for name in LICENSES {
                std::fs::write(src.join(name), name).expect("write licence");
            }
            let out = root.join(ASSET);
            let status = Command::new("tar")
                .arg("-czf")
                .arg(&out)
                .arg("-C")
                .arg(&src)
                .args([PROXY_BIN, "LICENSE", "LICENSE-ENGINE"])
                .status()
                .expect("run tar");
            assert!(status.success());
            std::fs::read(&out).expect("read archive")
        }

        fn serve(files: HashMap<String, Vec<u8>>) -> String {
            // nosemgrep: raw-socket-usage -- test-only stand-in for the GitHub release download
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let base = format!("http://{}", listener.local_addr().expect("server address"));
            std::thread::spawn(move || {
                for stream in listener.incoming().take(8) {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut request = String::new();
                    if reader.read_line(&mut request).is_err() {
                        continue;
                    }
                    let mut header = String::new();
                    while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                        header.clear();
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body) = match files.get(path) {
                        Some(body) => ("200 OK", body.clone()),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            base
        }

        fn release(bytes: Vec<u8>, listed: &str) -> String {
            let mut files = HashMap::new();
            files.insert(
                "/v9.9.9/checksums.txt".to_string(),
                format!("{listed}  {ASSET}\n").into_bytes(),
            );
            files.insert(format!("/v9.9.9/{ASSET}"), bytes);
            serve(files)
        }

        fn sha(bytes: &[u8]) -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }

        #[test]
        fn a_release_archive_is_installed_once_its_checksum_and_version_match() {
            let tmp = crate::core::test_isolation::tempdir();
            let bytes = archive(tmp.path(), "9.9.9");
            let listed = sha(&bytes);
            let base = release(bytes, &listed);
            let dest = tmp.path().join("data/bin").join(PROXY_BIN);
            install_from(&base, "9.9.9", ASSET, &dest).expect("install");
            assert_eq!(reported_version(&dest).as_deref(), Some("9.9.9"));
            let dir = dest.parent().expect("bin folder");
            assert!(dir.join("rtk-proxy-LICENSE-ENGINE").is_file());
            let left: Vec<_> = std::fs::read_dir(dir)
                .expect("list bin folder")
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with('.'))
                .collect();
            assert!(left.is_empty(), "download folder left behind: {left:?}");
        }

        #[test]
        fn an_archive_that_does_not_match_its_checksum_is_not_installed() {
            let tmp = crate::core::test_isolation::tempdir();
            let bytes = archive(tmp.path(), "9.9.9");
            let base = release(bytes, &"0".repeat(64));
            let dest = tmp.path().join("data/bin").join(PROXY_BIN);
            let err = install_from(&base, "9.9.9", ASSET, &dest).expect_err("refused");
            assert!(format!("{err:#}").contains("does not match the checksum"));
            assert!(!dest.exists());
        }

        #[test]
        fn a_binary_reporting_another_version_does_not_replace_the_installed_one() {
            let tmp = crate::core::test_isolation::tempdir();
            let bytes = archive(tmp.path(), "9.9.8");
            let listed = sha(&bytes);
            let base = release(bytes, &listed);
            let dest = tmp.path().join("data/bin").join(PROXY_BIN);
            std::fs::create_dir_all(dest.parent().expect("bin folder")).expect("create bin");
            std::fs::write(&dest, "kept").expect("write installed copy");
            let err = install_from(&base, "9.9.9", ASSET, &dest).expect_err("refused");
            assert!(format!("{err:#}").contains("reports version 9.9.8"));
            assert_eq!(std::fs::read_to_string(&dest).expect("read"), "kept");
        }

        #[test]
        fn a_version_without_a_release_installs_nothing() {
            let tmp = crate::core::test_isolation::tempdir();
            let base = serve(HashMap::new());
            let dest = tmp.path().join("data/bin").join(PROXY_BIN);
            let err = install_from(&base, "9.9.9", ASSET, &dest).expect_err("refused");
            assert!(format!("{err:#}").contains("no rtk-proxy release found for rtk 9.9.9"));
            assert!(!dest.exists());
        }
    }

    #[test]
    #[ignore = "downloads the published v1.2.1 rtk-proxy from GitHub"]
    fn the_published_rtk_proxy_installs() {
        let tmp = crate::core::test_isolation::tempdir();
        let dest = tmp.path().join("bin").join(PROXY_BIN);
        install_from(
            RELEASES,
            "1.2.1",
            asset().expect("a published target"),
            &dest,
        )
        .expect("install the v1.2.1 rtk-proxy");
        assert_eq!(reported_version(&dest).as_deref(), Some("1.2.1"));
    }
}
