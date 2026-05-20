use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const REPO: &str = "rtk-ai/rtk";
const GITHUB_API: &str = "https://api.github.com";
const GITHUB_WEB: &str = "https://github.com";

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub check: bool,
    pub tag: Option<String>,
    pub force: bool,
    pub yes: bool,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallKind {
    Homebrew,
    Cargo,
    SourceBuild,
    Direct,
}

pub fn run(opts: UpdateOptions) -> Result<i32> {
    let current_version = env!("CARGO_PKG_VERSION");
    let release = fetch_release(opts.tag.as_deref())?;
    let latest_version = release.tag_name.trim_start_matches('v');
    let newer = is_newer_version(latest_version, current_version);
    let target_differs = latest_version != current_version;
    let should_update = opts.force
        || if opts.tag.is_some() {
            target_differs
        } else {
            newer
        };

    if !should_update {
        println!("rtk already latest {}", current_version);
        return Ok(0);
    }

    if newer {
        println!(
            "rtk update available {} -> {}",
            current_version, latest_version
        );
    } else {
        println!(
            "rtk update target {} -> {}",
            current_version, latest_version
        );
    }
    if opts.check {
        return Ok(0);
    }

    let current_exe = std::env::current_exe().context("Could not resolve current rtk binary")?;
    match detect_install_kind(&current_exe) {
        InstallKind::Homebrew => {
            println!("rtk managed by Homebrew. Run: brew upgrade rtk");
            return Ok(0);
        }
        InstallKind::Cargo => {
            println!(
                "rtk managed by Cargo. Run: cargo install --git https://github.com/rtk-ai/rtk --force"
            );
            return Ok(0);
        }
        InstallKind::SourceBuild => {
            println!(
                "rtk running from source build. Run: cargo install --path . --force or cargo install --git https://github.com/rtk-ai/rtk --force"
            );
            return Ok(0);
        }
        InstallKind::Direct => {}
    }

    #[cfg(target_os = "windows")]
    {
        let asset = select_asset(std::env::consts::OS, std::env::consts::ARCH, &release)
            .context("No release asset for this platform")?;
        let checksum_asset = release
            .assets
            .iter()
            .find(|asset| asset.name == "checksums.txt")
            .context("Release missing checksums.txt")?;
        let archive = download(&asset.browser_download_url)?;
        let checksums = String::from_utf8(download(&checksum_asset.browser_download_url)?)
            .context("checksums.txt is not UTF-8")?;
        verify_checksum(&archive, &asset.name, &checksums)?;
        println!(
            "rtk downloaded and verified {}. Windows self-replace not supported; replace {} manually from release zip.",
            asset.name,
            current_exe.display()
        );
        return Ok(0);
    }

    #[cfg(not(target_os = "windows"))]
    {
        if !opts.yes && !confirm_update(current_version, latest_version, &current_exe)? {
            println!("rtk update cancelled");
            return Ok(1);
        }

        let asset = select_asset(std::env::consts::OS, std::env::consts::ARCH, &release)
            .context("No release asset for this platform")?;
        let checksum_asset = release
            .assets
            .iter()
            .find(|asset| asset.name == "checksums.txt")
            .context("Release missing checksums.txt")?;

        let archive = download(&asset.browser_download_url)?;
        let checksums = String::from_utf8(download(&checksum_asset.browser_download_url)?)
            .context("checksums.txt is not UTF-8")?;
        verify_checksum(&archive, &asset.name, &checksums)?;

        let binary = extract_rtk_from_tar_gz(&archive)?;
        replace_binary(&current_exe, &binary)?;
        println!("rtk updated {} -> {}", current_version, latest_version);
        Ok(0)
    }
}

fn fetch_release(tag: Option<&str>) -> Result<Release> {
    match tag {
        Some(tag) => fetch_release_from_api(Some(tag)),
        None => match fetch_latest_tag_from_redirect() {
            Ok(tag) => Ok(release_from_known_assets(&tag)),
            Err(_) => fetch_release_from_api(None),
        },
    }
}

fn fetch_latest_tag_from_redirect() -> Result<String> {
    let url = format!("{}/{}/releases/latest", GITHUB_WEB, REPO);
    let response = ureq::head(&url)
        .set("User-Agent", concat!("rtk/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| anyhow!("Failed to resolve latest release redirect: {}", e))?;
    parse_tag_from_release_url(response.get_url()).context("Latest release redirect had no tag")
}

fn parse_tag_from_release_url(url: &str) -> Option<String> {
    url.split("/releases/tag/")
        .nth(1)
        .and_then(|tail| tail.split(['?', '#']).next())
        .filter(|tag| !tag.trim().is_empty())
        .map(|tag| tag.trim().to_string())
}

fn fetch_release_from_api(tag: Option<&str>) -> Result<Release> {
    let path = match tag {
        Some(tag) => format!("/repos/{}/releases/tags/{}", REPO, tag),
        None => format!("/repos/{}/releases/latest", REPO),
    };
    let url = format!("{}{}", GITHUB_API, path);
    let body = ureq::get(&url)
        .set("User-Agent", concat!("rtk/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| anyhow!("Failed to fetch release metadata: {}", e))?
        .into_string()
        .context("Failed to read release metadata")?;
    serde_json::from_str(&body).context("Failed to parse release metadata")
}

fn release_from_known_assets(tag: &str) -> Release {
    const ASSETS: &[&str] = &[
        "checksums.txt",
        "rtk-aarch64-apple-darwin.tar.gz",
        "rtk-aarch64-unknown-linux-gnu.tar.gz",
        "rtk-x86_64-apple-darwin.tar.gz",
        "rtk-x86_64-pc-windows-msvc.zip",
        "rtk-x86_64-unknown-linux-musl.tar.gz",
    ];
    Release {
        tag_name: tag.to_string(),
        assets: ASSETS
            .iter()
            .map(|name| ReleaseAsset {
                name: (*name).to_string(),
                browser_download_url: format!(
                    "{}/{}/releases/download/{}/{}",
                    GITHUB_WEB, REPO, tag, name
                ),
            })
            .collect(),
    }
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut reader = ureq::get(url)
        .set("User-Agent", concat!("rtk/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| anyhow!("Failed to download {}: {}", url, e))?
        .into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("Failed to read {}", url))?;
    Ok(bytes)
}

#[cfg(not(target_os = "windows"))]
fn confirm_update(current: &str, latest: &str, path: &Path) -> Result<bool> {
    print!(
        "Update rtk {} -> {} at {}? [y/N] ",
        current,
        latest,
        path.display()
    );
    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("Failed to read confirmation")?;
    let answer = input.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

fn select_asset<'a>(os: &str, arch: &str, release: &'a Release) -> Option<&'a ReleaseAsset> {
    let target = target_asset_name(os, arch)?;
    release.assets.iter().find(|asset| asset.name == target)
}

fn target_asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("rtk-x86_64-unknown-linux-musl.tar.gz"),
        ("linux", "aarch64") => Some("rtk-aarch64-unknown-linux-gnu.tar.gz"),
        ("macos", "x86_64") => Some("rtk-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Some("rtk-aarch64-apple-darwin.tar.gz"),
        ("windows", "x86_64") => Some("rtk-x86_64-pc-windows-msvc.zip"),
        _ => None,
    }
}

fn verify_checksum(bytes: &[u8], asset_name: &str, checksums: &str) -> Result<()> {
    let expected = parse_checksum(checksums, asset_name)
        .with_context(|| format!("checksums.txt missing {}", asset_name))?;
    let actual = sha256_hex(bytes);
    if actual != expected {
        bail!(
            "Checksum mismatch for {}: expected {}, got {}",
            asset_name,
            expected,
            actual
        );
    }
    Ok(())
}

fn parse_checksum(checksums: &str, asset_name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        if name == asset_name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(not(target_os = "windows"))]
fn extract_rtk_from_tar_gz(archive: &[u8]) -> Result<Vec<u8>> {
    let decoder = GzDecoder::new(Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let mut found = None;

    for entry in tar.entries().context("Failed to read release archive")? {
        let mut entry = entry.context("Failed to read archive entry")?;
        let path = entry.path().context("Failed to read archive entry path")?;
        validate_archive_entry(&path, entry.header().entry_type())?;

        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .context("Failed to extract rtk binary")?;
        found = Some(bytes);
    }

    found.context("Release archive did not contain rtk binary")
}

fn validate_archive_entry(path: &Path, entry_type: tar::EntryType) -> Result<()> {
    validate_archive_path(path)?;
    if !entry_type.is_file() {
        bail!("Blocked non-file archive entry: {}", path.display());
    }
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.components().count() != 1 {
        bail!("Blocked unsafe archive path: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::Normal(name) if name == OsStr::new("rtk") => {}
            _ => bail!("Blocked unsafe archive path: {}", path.display()),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_binary(current_exe: &Path, binary: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let parent = current_exe
        .parent()
        .context("Current binary has no parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;
    temp.write_all(binary)
        .context("Failed to write new rtk binary")?;
    temp.as_file_mut()
        .sync_all()
        .context("Failed to sync new rtk binary")?;

    let mut perms = temp.as_file().metadata()?.permissions();
    perms.set_mode(0o755);
    temp.as_file()
        .set_permissions(perms)
        .context("Failed to set executable permissions")?;

    temp.persist(current_exe).map_err(|e| {
        anyhow!(
            "Failed to install new binary at {}: {}",
            current_exe.display(),
            e.error
        )
    })?;
    Ok(())
}

fn detect_install_kind(path: &Path) -> InstallKind {
    let homebrew_paths = homebrew_paths();
    detect_install_kind_with_paths(path, &homebrew_paths, cargo_home().as_deref())
}

fn detect_install_kind_with_paths(
    path: &Path,
    homebrew_paths: &[PathBuf],
    cargo_home: Option<&Path>,
) -> InstallKind {
    if is_homebrew_path(path, homebrew_paths) {
        return InstallKind::Homebrew;
    }
    if is_source_build(path) {
        return InstallKind::SourceBuild;
    }
    if is_cargo_path(path, cargo_home) {
        return InstallKind::Cargo;
    }
    InstallKind::Direct
}

fn homebrew_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for args in [["--prefix", "rtk"], ["--cellar", "rtk"]] {
        if let Some(path) = run_brew_path(&args) {
            paths.push(path);
        }
    }
    paths
}

fn run_brew_path(args: &[&str]) -> Option<PathBuf> {
    let output = Command::new("brew").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn is_homebrew_path(path: &Path, homebrew_paths: &[PathBuf]) -> bool {
    let normalized_path = normalize_path(path);
    homebrew_paths.iter().any(|brew_path| {
        let normalized_brew = normalize_path(brew_path);
        normalized_path.starts_with(&normalized_brew)
    }) || has_cellar_rtk_components(path)
}

fn has_cellar_rtk_components(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    components
        .windows(2)
        .any(|pair| pair[0] == OsStr::new("Cellar") && pair[1] == OsStr::new("rtk"))
}

fn is_cargo_path(path: &Path, cargo_home: Option<&Path>) -> bool {
    let Some(cargo_home) = cargo_home else {
        return false;
    };
    normalize_path(path).starts_with(normalize_path(&cargo_home.join("bin")))
}

fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_source_build(path: &Path) -> bool {
    let mut saw_target = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        if saw_target && (name == OsStr::new("debug") || name == OsStr::new("release")) {
            return true;
        }
        saw_target = name == OsStr::new("target");
    }
    false
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

fn parse_version(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(|c: char| !(c.is_ascii_digit()))
        .filter(|part| !part.is_empty())
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    fn tar_gz_entry(path: &str, entry_type: tar::EntryType, data: &[u8]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(entry_type);
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
            builder.finish().unwrap();
        }

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[cfg(not(target_os = "windows"))]
    fn tar_gz_with_rewritten_path(from: &str, to: &str, data: &[u8]) -> Vec<u8> {
        let archive = tar_gz_entry(from, tar::EntryType::file(), data);
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        let mut tar_bytes = Vec::new();
        decoder.read_to_end(&mut tar_bytes).unwrap();

        tar_bytes[0..100].fill(0);
        tar_bytes[0..to.len()].copy_from_slice(to.as_bytes());
        tar_bytes[148..156].fill(b' ');
        let checksum: u32 = tar_bytes[0..512].iter().map(|byte| *byte as u32).sum();
        let checksum_text = format!("{:06o}\0 ", checksum);
        tar_bytes[148..156].copy_from_slice(checksum_text.as_bytes());

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn update_version_compare_handles_v_prefix() {
        assert!(is_newer_version("v0.39.0", "0.38.9"));
        assert!(is_newer_version("0.40.0", "0.39.9"));
        assert!(!is_newer_version("v0.39.0", "0.39.0"));
        assert!(!is_newer_version("v0.38.9", "0.39.0"));
    }

    #[test]
    fn update_parses_latest_tag_from_redirect_url() {
        assert_eq!(
            parse_tag_from_release_url("https://github.com/rtk-ai/rtk/releases/tag/v0.39.0"),
            Some("v0.39.0".to_string())
        );
        assert_eq!(
            parse_tag_from_release_url("https://github.com/rtk-ai/rtk/releases"),
            None
        );
    }

    #[test]
    fn update_known_release_assets_match_platforms() {
        let release = release_from_known_assets("v0.39.0");
        assert_eq!(release.tag_name, "v0.39.0");
        assert!(select_asset("linux", "x86_64", &release).is_some());
        assert!(release
            .assets
            .iter()
            .any(|asset| asset.name == "checksums.txt"));
    }

    #[test]
    fn update_selects_platform_assets() {
        assert_eq!(
            target_asset_name("linux", "x86_64"),
            Some("rtk-x86_64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(
            target_asset_name("macos", "aarch64"),
            Some("rtk-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            target_asset_name("windows", "x86_64"),
            Some("rtk-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(target_asset_name("freebsd", "x86_64"), None);
    }

    #[test]
    fn update_parses_checksums() {
        let checksums = "abc  other\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  rtk-x86_64-unknown-linux-musl.tar.gz\n";
        assert_eq!(
            parse_checksum(checksums, "rtk-x86_64-unknown-linux-musl.tar.gz"),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string())
        );
        assert_eq!(parse_checksum(checksums, "missing.tar.gz"), None);
    }

    #[test]
    fn update_verifies_checksum() {
        let bytes = b"hello";
        let hash = sha256_hex(bytes);
        let checksums = format!("{}  rtk-test.tar.gz\n", hash);
        assert!(verify_checksum(bytes, "rtk-test.tar.gz", &checksums).is_ok());
        assert!(verify_checksum(b"bye", "rtk-test.tar.gz", &checksums).is_err());
    }

    #[test]
    fn update_rejects_unsafe_archive_paths() {
        assert!(validate_archive_path(Path::new("rtk")).is_ok());
        assert!(validate_archive_path(Path::new("../rtk")).is_err());
        assert!(validate_archive_path(Path::new("dir/rtk")).is_err());
        assert!(validate_archive_path(Path::new("/tmp/rtk")).is_err());
        assert!(validate_archive_path(Path::new("other")).is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn update_extracts_valid_tar_gz() {
        let archive = tar_gz_entry("rtk", tar::EntryType::file(), b"new-binary");
        assert_eq!(extract_rtk_from_tar_gz(&archive).unwrap(), b"new-binary");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn update_rejects_tar_gz_path_traversal() {
        let archive = tar_gz_with_rewritten_path("aaa", "../rtk", b"bad");
        assert!(extract_rtk_from_tar_gz(&archive).is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn update_rejects_tar_gz_symlink() {
        let archive = tar_gz_entry("rtk", tar::EntryType::symlink(), b"target");
        assert!(extract_rtk_from_tar_gz(&archive).is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn update_replaces_binary_atomically() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rtk");
        std::fs::write(&path, b"old").unwrap();
        replace_binary(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0);
    }

    #[test]
    fn update_detects_install_kinds_from_paths() {
        let brew_roots = vec![
            PathBuf::from("/opt/homebrew/opt/rtk"),
            PathBuf::from("/opt/homebrew/Cellar/rtk"),
        ];
        assert_eq!(
            detect_install_kind_with_paths(
                Path::new("/opt/homebrew/Cellar/rtk/0.39.0/bin/rtk"),
                &brew_roots,
                Some(Path::new("/home/me/.cargo"))
            ),
            InstallKind::Homebrew
        );
        assert_eq!(
            detect_install_kind_with_paths(
                Path::new("/home/me/.cargo/bin/rtk"),
                &[],
                Some(Path::new("/home/me/.cargo"))
            ),
            InstallKind::Cargo
        );
        assert_eq!(
            detect_install_kind_with_paths(
                Path::new("/repo/target/debug/rtk"),
                &[],
                Some(Path::new("/home/me/.cargo"))
            ),
            InstallKind::SourceBuild
        );
        assert_eq!(
            detect_install_kind_with_paths(
                Path::new("/home/me/.local/bin/rtk"),
                &[],
                Some(Path::new("/home/me/.cargo"))
            ),
            InstallKind::Direct
        );
    }
}
