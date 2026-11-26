//! Local services a pipeline declares under `[dependency.<name>]`, such as a
//! decision model its classifier asks. rtk installs one only after the user
//! says yes in a terminal, or, with no terminal, when the user named a pipeline
//! from the reviewed catalog with `rtk init -g --pipeline <name>`; no flag or
//! variable answers for any other source. The recipe comes from the pipeline
//! file: an exact package version or file whose hash rtk checks before
//! anything runs. A pipeline from the reviewed catalog says so; any other
//! source is installed at the user's own risk.
//!
//! What was accepted is stored with the install, and only that copy is ever
//! started: when a call to an upstream linked with `dependency = "<name>"`
//! fails, [`wake`] starts it in the background for the calls that follow and
//! leaves one message for the user. A hosted service (Jev, hosted Laya, any
//! API) is never installed, started or stopped. Installs stay until the full
//! `rtk init -g --uninstall`, which stops them and removes them.

use crate::core::user_dirs;
use anyhow::{Context, Result};
use rtk_engine::pipeline::{DependencyDef, Installer, Resolved, UpstreamDef};
use sha2::{Digest, Sha256, Sha512};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MARKER: &str = ".rtk-dependency.json";
const PID: &str = ".pid";
const STAMP: &str = ".start-attempt";
const FAILED: &str = "failed";
const LOG: &str = "service.log";
const RETRY_AFTER_START: Duration = Duration::from_secs(60);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(1800);
const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Ready,
    Starting(Option<String>),
    Foreign,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    AlreadyRunning,
    Launched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Missing,
    Outdated,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Review {
    Reviewed(String),
    Unreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ask,
    Auto,
    NoTerminal,
}

struct Installed {
    fingerprint: String,
    def: DependencyDef,
    dir: PathBuf,
}

pub fn root() -> Option<PathBuf> {
    user_dirs::data().map(|d| d.join("dependencies"))
}

fn dir(name: &str) -> Option<PathBuf> {
    let safe = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !safe {
        return None;
    }
    root().map(|r| r.join(name))
}

pub fn fingerprint(def: &DependencyDef) -> String {
    hex(&Sha256::digest(recipe_json(def).to_string().as_bytes()))
}

fn recipe_json(def: &DependencyDef) -> serde_json::Value {
    let installer = match &def.installer {
        Some(Installer::Npm {
            package,
            integrity,
            node,
        }) => serde_json::json!({ "npm": package, "integrity": integrity, "node": node }),
        Some(Installer::Pip {
            requirements,
            python,
        }) => serde_json::json!({ "pip": requirements, "python": python }),
        Some(Installer::Binary { platforms }) => {
            let map: serde_json::Map<String, serde_json::Value> = platforms
                .iter()
                .map(|(platform, bin)| {
                    (
                        platform.clone(),
                        serde_json::json!({ "url": bin.url, "sha256": bin.sha256 }),
                    )
                })
                .collect();
            serde_json::json!({ "binary": map })
        }
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "about": def.about,
        "needs": def.needs,
        "installer": installer,
        "files": def.files,
        "env": def.env,
        "run": def.run,
        "listen": def.listen,
        "health": def.health,
    })
}

fn recipe_from_json(value: &serde_json::Value) -> Option<DependencyDef> {
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let strings = |key: &str| -> BTreeMap<String, String> {
        value
            .get(key)
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let installer = value.get("installer")?;
    let installer = if let Some(package) = installer.get("npm").and_then(|v| v.as_str()) {
        Installer::Npm {
            package: package.to_string(),
            integrity: installer.get("integrity")?.as_str()?.to_string(),
            node: installer
                .get("node")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok()),
        }
    } else if let Some(items) = installer.get("pip").and_then(|v| v.as_array()) {
        Installer::Pip {
            requirements: items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            python: installer
                .get("python")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }
    } else {
        let map = installer.get("binary")?.as_object()?;
        Installer::Binary {
            platforms: map
                .iter()
                .filter_map(|(platform, bin)| {
                    Some((
                        platform.clone(),
                        rtk_engine::pipeline::BinaryDef {
                            url: bin.get("url")?.as_str()?.to_string(),
                            sha256: bin.get("sha256")?.as_str()?.to_string(),
                        },
                    ))
                })
                .collect(),
        }
    };
    Some(DependencyDef {
        about: text("about"),
        needs: text("needs"),
        installer: Some(installer),
        files: strings("files"),
        env: strings("env"),
        run: value
            .get("run")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        listen: text("listen"),
        health: text("health"),
        unknown: Vec::new(),
        line: None,
    })
}

fn installed(name: &str) -> Option<Installed> {
    let dir = dir(name)?;
    let text = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let marker: serde_json::Value = serde_json::from_str(&text).ok()?;
    if marker.get("name")?.as_str()? != name {
        return None;
    }
    Some(Installed {
        fingerprint: marker.get("fingerprint")?.as_str()?.to_string(),
        def: recipe_from_json(marker.get("recipe")?)?,
        dir,
    })
}

pub fn state(name: &str, def: &DependencyDef) -> State {
    match installed(name) {
        None => State::Missing,
        Some(found) if found.fingerprint == fingerprint(def) => State::Installed,
        Some(_) => State::Outdated,
    }
}

pub fn platform() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

pub fn installable(def: &DependencyDef) -> std::result::Result<(), String> {
    if !def.unknown.is_empty() {
        return Err(format!(
            "its recipe uses keys this rtk does not know: {}",
            def.unknown.join(", ")
        ));
    }
    match &def.installer {
        None => Err("its recipe names no installer this rtk knows".to_string()),
        Some(Installer::Binary { platforms }) if !platforms.contains_key(&platform()) => Err(
            format!("it ships no binary for this computer ({})", platform()),
        ),
        Some(_) => Ok(()),
    }
}

pub fn install(name: &str, def: &DependencyDef) -> Result<()> {
    if let Err(reason) = installable(def) {
        anyhow::bail!("rtk cannot install {name}: {reason}");
    }
    let root = root().context("no data directory for dependencies")?;
    let dir = dir(name).context("no data directory for dependencies")?;
    let previous = root.join(format!(".previous-{name}"));
    if previous.exists() {
        remove_inside_root(&previous)?;
    }
    let replacing = dir.exists();
    if replacing {
        stop(name)?;
        std::fs::rename(&dir, &previous)
            .with_context(|| format!("move the installed {name} aside"))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let Err(error) = install_into(name, &dir, def) else {
        if replacing {
            let _ = remove_inside_root(&previous);
        }
        return Ok(());
    };
    let log = dir.join(LOG);
    let logged = std::fs::metadata(&log).is_ok_and(|m| m.len() > 0);
    if replacing {
        let kept = root.join(format!(".failed-{name}.log"));
        let kept_log = logged && std::fs::rename(&log, &kept).is_ok();
        remove_inside_root(&dir)?;
        std::fs::rename(&previous, &dir)
            .with_context(|| format!("put the previous {name} back"))?;
        let note = if kept_log {
            format!(
                "the previous {name} was put back; the failed install's log is {}",
                kept.display()
            )
        } else {
            format!("the previous {name} was put back")
        };
        return Err(error.context(note));
    }
    if !logged {
        let _ = remove_inside_root(&dir);
    }
    Err(error)
}

pub fn listen_answers(def: &DependencyDef) -> bool {
    def.listen.as_deref().is_some_and(address_answers)
}

pub fn address_answers(address: &str) -> bool {
    socket_addr(address)
        .and_then(|addr| connect(&addr))
        .is_some()
}

fn connect(addr: &SocketAddr) -> Option<TcpStream> {
    // nosemgrep: raw-socket-usage
    TcpStream::connect_timeout(addr, Duration::from_millis(300)).ok()
}

fn command_for(program: &Path) -> Result<Command> {
    let name = program
        .to_str()
        .with_context(|| format!("{} is not a UTF-8 path", program.display()))?;
    Ok(crate::core::utils::resolved_command(name))
}

fn install_into(name: &str, dir: &Path, def: &DependencyDef) -> Result<()> {
    let dir = dir.to_path_buf();
    for (file, text) in &def.files {
        std::fs::write(dir.join(file), text).with_context(|| format!("write {file}"))?;
    }
    match &def.installer {
        Some(Installer::Npm {
            package,
            integrity,
            node,
        }) => install_npm(name, &dir, package, integrity, *node)?,
        Some(Installer::Pip {
            requirements,
            python,
        }) => install_pip(&dir, requirements, python.as_deref())?,
        Some(Installer::Binary { platforms }) => {
            let bin = platforms
                .get(&platform())
                .context("no binary for this computer")?;
            install_binary(&dir, &bin.url, &bin.sha256)?;
        }
        None => anyhow::bail!("no installer"),
    }
    let marker = serde_json::json!({
        "name": name,
        "fingerprint": fingerprint(def),
        "recipe": recipe_json(def),
    });
    std::fs::write(dir.join(MARKER), format!("{marker}\n"))
        .with_context(|| format!("record the install of {name}"))?;
    Ok(())
}

fn install_npm(
    name: &str,
    dir: &Path,
    package: &str,
    integrity: &str,
    min_node: Option<u32>,
) -> Result<()> {
    let (node, _) = find_node(min_node.unwrap_or(0)).with_context(|| node_missing(min_node))?;
    let bin = node.parent().context("node has no parent directory")?;
    let npm = bin.join(if cfg!(windows) { "npm.cmd" } else { "npm" });
    if !npm.is_file() {
        anyhow::bail!("npm not found next to {}", node.display());
    }
    std::fs::write(
        dir.join("package.json"),
        format!("{{\n  \"name\": \"rtk-dependency-{name}\",\n  \"private\": true\n}}\n"),
    )
    .context("write package.json")?;
    let path = path_with(&[bin.to_path_buf()])?;
    let out = command_for(&npm)?
        .args(["pack", package, "--silent", "--pack-destination", "."])
        .current_dir(dir)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stderr(open_log(dir)?)
        .output()
        .with_context(|| format!("run {}", npm.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "npm could not download {package}; see {}",
            dir.join(LOG).display()
        );
    }
    let tarball = String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_string();
    let plain = tarball.ends_with(".tgz")
        && tarball
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !plain {
        anyhow::bail!("npm pack gave an unexpected file name `{tarball}`");
    }
    let file = dir.join(&tarball);
    let actual = format!("sha512-{}", base64(&hash_file::<Sha512>(&file)?));
    if actual != integrity {
        let _ = std::fs::remove_file(&file);
        anyhow::bail!(
            "{package} does not match the hash its pipeline declares (declared {integrity}, downloaded {actual}); nothing was installed"
        );
    }
    let status = command_for(&npm)?
        .args([
            "install",
            &format!("./{tarball}"),
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        ])
        .current_dir(dir)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(open_log(dir)?)
        .stderr(open_log(dir)?)
        .status()
        .with_context(|| format!("run {}", npm.display()))?;
    let _ = std::fs::remove_file(&file);
    if !status.success() {
        anyhow::bail!(
            "npm install {package} failed ({status}); see {}",
            dir.join(LOG).display()
        );
    }
    Ok(())
}

fn install_pip(dir: &Path, requirements: &[String], min_python: Option<&str>) -> Result<()> {
    let python = find_python(min_python).with_context(|| match min_python {
        Some(v) => format!("Python {v} or newer was not found"),
        None => "Python 3 was not found".to_string(),
    })?;
    let status = command_for(&python)?
        .args(["-m", "venv", "venv"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(open_log(dir)?)
        .stderr(open_log(dir)?)
        .status()
        .with_context(|| format!("run {}", python.display()))?;
    if !status.success() {
        anyhow::bail!(
            "could not create a Python environment; see {}",
            dir.join(LOG).display()
        );
    }
    std::fs::write(
        dir.join("requirements.txt"),
        format!("{}\n", requirements.join("\n")),
    )
    .context("write requirements.txt")?;
    let venv_python = venv_bin(dir).join(if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    });
    let status = command_for(&venv_python)?
        .args([
            "-m",
            "pip",
            "install",
            "--require-hashes",
            "--no-input",
            "--disable-pip-version-check",
            "-r",
            "requirements.txt",
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(open_log(dir)?)
        .stderr(open_log(dir)?)
        .status()
        .with_context(|| format!("run {}", venv_python.display()))?;
    if !status.success() {
        anyhow::bail!(
            "pip install failed ({status}); a package may not match its declared hash, see {}",
            dir.join(LOG).display()
        );
    }
    Ok(())
}

fn install_binary(dir: &Path, url: &str, sha256: &str) -> Result<()> {
    let name = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .filter(|n| {
            !n.is_empty()
                && !n.starts_with('.')
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        })
        .unwrap_or("download")
        .to_string();
    let file = dir.join(&name);
    let actual = download(url, &file)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(&file);
        anyhow::bail!(
            "{url} does not match the hash its pipeline declares (declared {sha256}, downloaded {actual}); nothing was installed"
        );
    }
    let lower = name.to_ascii_lowercase();
    let archive = [".tar.gz", ".tgz", ".tar.xz", ".tar", ".zip"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if !archive {
        make_executable(&file)?;
        return Ok(());
    }
    let unzip = lower.ends_with(".zip") && cfg!(target_os = "linux");
    let status = if unzip {
        Command::new("unzip")
            .args(["-q", "-o"])
            .arg(&file)
            .arg("-d")
            .arg(dir)
            .status()
    } else {
        Command::new("tar")
            .arg("-xf")
            .arg(&file)
            .arg("-C")
            .arg(dir)
            .status()
    }
    .with_context(|| format!("unpack {name}"))?;
    let _ = std::fs::remove_file(&file);
    if !status.success() {
        anyhow::bail!("could not unpack {name}");
    }
    Ok(())
}

fn download(url: &str, file: &Path) -> Result<String> {
    let mut response = crate::core::http::get_download(url)?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let mut out =
        std::fs::File::create(file).with_context(|| format!("create {}", file.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("download {url}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .with_context(|| format!("write {}", file.display()))?;
    }
    Ok(hex(&hasher.finalize()))
}

#[cfg(unix)]
fn make_executable(file: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("make {} executable", file.display()))
}

#[cfg(not(unix))]
fn make_executable(_file: &Path) -> Result<()> {
    Ok(())
}

fn hash_file<D: Digest>(file: &Path) -> Result<Vec<u8>> {
    let mut input =
        std::fs::File::open(file).with_context(|| format!("open {}", file.display()))?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buf)
            .with_context(|| format!("read {}", file.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let digits = [18, 12, 6, 0].map(|shift| TABLE[((n >> shift) & 63) as usize] as char);
        out.push(digits[0]);
        out.push(digits[1]);
        out.push(if chunk.len() > 1 { digits[2] } else { '=' });
        out.push(if chunk.len() > 2 { digits[3] } else { '=' });
    }
    out
}

fn remove_inside_root(dir: &Path) -> Result<()> {
    let root = root().context("no data directory for dependencies")?;
    if dir.parent() != Some(root.as_path()) {
        anyhow::bail!(
            "refusing to remove {}: not an rtk dependency",
            dir.display()
        );
    }
    std::fs::remove_dir_all(dir).with_context(|| format!("remove {}", dir.display()))
}

fn node_missing(min: Option<u32>) -> String {
    match min {
        Some(major) => format!("Node.js {major} or newer was not found"),
        None => "Node.js was not found".to_string(),
    }
}

pub fn start(name: &str) -> Result<Started> {
    let found = installed(name).with_context(|| format!("{name} is not installed"))?;
    let listen = found.def.listen.clone().unwrap_or_default();
    match health_of(&found.dir, &found.def) {
        Health::Ready | Health::Starting(_) => return Ok(Started::AlreadyRunning),
        Health::Foreign => {
            anyhow::bail!("another program answers on {listen}, so {name} cannot start there")
        }
        Health::Down => {}
    }
    let (program, path) = resolve_program(&found)?;
    let log = open_log(&found.dir)?;
    let mut command = command_for(&program)?;
    command
        .args(found.def.run.iter().skip(1))
        .current_dir(&found.dir)
        .env("PATH", path)
        .envs(&found.def.env)
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("clone the service log")?)
        .stderr(log);
    detach(&mut command);
    let child = command
        .spawn()
        .with_context(|| format!("start {}", program.display()))?;
    std::fs::write(found.dir.join(PID), format!("{}\n", child.id()))
        .context("record the service process")?;
    Ok(Started::Launched)
}

fn resolve_program(found: &Installed) -> Result<(PathBuf, std::ffi::OsString)> {
    let mut dirs = vec![
        found.dir.clone(),
        found.dir.join("bin"),
        venv_bin(&found.dir),
        found.dir.join("node_modules").join(".bin"),
    ];
    if let Some(Installer::Npm { node, .. }) = &found.def.installer {
        let (path, _) = find_node(node.unwrap_or(0)).with_context(|| node_missing(*node))?;
        if let Some(bin) = path.parent() {
            dirs.push(bin.to_path_buf());
        }
    }
    let path = path_with(&dirs)?;
    let program = found.def.run.first().context("the recipe has no `run`")?;
    if program.contains('/') || program.contains('\\') {
        let candidate = found.dir.join(program);
        if candidate.is_file() {
            return Ok((candidate, path));
        }
        anyhow::bail!("{program} was not found in {}", found.dir.display());
    }
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"]
            .iter()
            .map(|ext| format!("{program}{ext}"))
            .collect()
    } else {
        vec![program.clone()]
    };
    std::env::split_paths(&path)
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|candidate| candidate.is_file())
        .map(|candidate| (candidate, path.clone()))
        .with_context(|| format!("{program} was not found"))
}

fn venv_bin(dir: &Path) -> PathBuf {
    dir.join("venv")
        .join(if cfg!(windows) { "Scripts" } else { "bin" })
}

fn socket_addr(listen: &str) -> Option<SocketAddr> {
    let (host, port) = listen.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    match host {
        "127.0.0.1" | "localhost" => Some(SocketAddr::from(([127, 0, 0, 1], port))),
        "[::1]" => Some(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))),
        _ => None,
    }
}

fn health_of(dir: &Path, def: &DependencyDef) -> Health {
    let Some(addr) = def.listen.as_deref().and_then(socket_addr) else {
        return Health::Down;
    };
    let Some(stream) = connect(&addr) else {
        return if ours_running(dir, def) {
            Health::Starting(None)
        } else {
            Health::Down
        };
    };
    if !ours_running(dir, def) {
        return Health::Foreign;
    }
    let Some(path) = def.health.as_deref() else {
        return Health::Ready;
    };
    match http_get(stream, path) {
        Some((200..=299, _)) => Health::Ready,
        Some((503, body)) => Health::Starting(short_line(&body)),
        Some(_) => Health::Ready,
        None => Health::Starting(None),
    }
}

fn http_get(mut stream: TcpStream, path: &str) -> Option<(u16, String)> {
    let timeout = Some(Duration::from_millis(800));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = Vec::new();
    stream.take(16 * 1024).read_to_end(&mut raw).ok()?;
    parse_response(&String::from_utf8_lossy(&raw))
}

fn parse_response(raw: &str) -> Option<(u16, String)> {
    let (head, body) = raw.split_once("\r\n\r\n")?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((status, body.to_string()))
}

fn short_line(body: &str) -> Option<String> {
    let line = body.trim();
    let plain = !line.is_empty()
        && line.len() <= 120
        && !line.contains('\n')
        && !line.starts_with('{')
        && !line.starts_with('<');
    plain.then(|| crate::core::utils::sanitize_for_terminal(line))
}

fn read_pid(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join(PID))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn ours_running(dir: &Path, def: &DependencyDef) -> bool {
    read_pid(dir).is_some_and(|pid| runs(pid, def))
}

#[cfg(unix)]
fn runs(pid: u32, def: &DependencyDef) -> bool {
    let Some(program) = def
        .run
        .first()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
    else {
        return false;
    };
    let Ok(out) = crate::core::utils::resolved_command("ps")
        .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    let line = String::from_utf8_lossy(&out.stdout);
    out.status.success()
        && !line.contains("<defunct>")
        && line.contains(program)
        && def
            .run
            .iter()
            .skip(1)
            .all(|arg| arg.is_empty() || line.contains(arg.as_str()))
}

#[cfg(windows)]
fn runs(pid: u32, _def: &DependencyDef) -> bool {
    pid_alive(pid)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    crate::core::utils::resolved_command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")))
}

#[cfg(not(any(unix, windows)))]
fn runs(_pid: u32, _def: &DependencyDef) -> bool {
    false
}

#[cfg(unix)]
fn terminate(pid: u32, force: bool) -> bool {
    let signal = if force { "KILL" } else { "TERM" };
    let send = |target: String| {
        crate::core::utils::resolved_command("kill")
            .args(["-s", signal, "--", &target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    send(format!("-{pid}")) || send(pid.to_string())
}

#[cfg(windows)]
fn terminate(pid: u32, _force: bool) -> bool {
    crate::core::utils::resolved_command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(any(unix, windows)))]
fn terminate(_pid: u32, _force: bool) -> bool {
    false
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

pub fn stop(name: &str) -> Result<bool> {
    let Some(found) = installed(name) else {
        return Ok(false);
    };
    let Some(pid) = read_pid(&found.dir).filter(|pid| runs(*pid, &found.def)) else {
        let _ = std::fs::remove_file(found.dir.join(PID));
        return Ok(false);
    };
    for (force, wait) in [
        (false, Duration::from_secs(3)),
        (true, Duration::from_secs(1)),
    ] {
        if !terminate(pid, force) && runs(pid, &found.def) {
            anyhow::bail!("could not stop {name} (process {pid})");
        }
        let deadline = Instant::now() + wait;
        while runs(pid, &found.def) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !runs(pid, &found.def) {
            let _ = std::fs::remove_file(found.dir.join(PID));
            return Ok(true);
        }
    }
    anyhow::bail!("{name} (process {pid}) is still running after rtk asked it to stop")
}

pub fn wake(upstream: &UpstreamDef) {
    let Some(name) = upstream.dependency.as_deref() else {
        return;
    };
    let Some(found) = installed(name) else {
        return;
    };
    if recently_claimed(&found.dir)
        || health_of(&found.dir, &found.def) != Health::Down
        || !claim_start(&found.dir)
    {
        return;
    }
    let message = match start(name) {
        Ok(Started::Launched) => format!(
            "rtk started {name}, a local service your pipeline uses. Until it is ready, tool calls run without the rules that need it."
        ),
        Ok(Started::AlreadyRunning) => return,
        Err(e) => {
            let _ = std::fs::write(found.dir.join(STAMP), format!("{FAILED}\n"));
            format!(
                "rtk could not start {name}, a local service your pipeline uses ({e:#}). Tool calls run without the rules that need it; `rtk pipeline status` shows more."
            )
        }
    };
    crate::core::notice::push(message);
}

fn recently_claimed(dir: &Path) -> bool {
    let wait = if last_start_failed(dir) {
        RETRY_AFTER_FAILURE
    } else {
        RETRY_AFTER_START
    };
    std::fs::metadata(dir.join(STAMP))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < wait)
}

fn claim_start(dir: &Path) -> bool {
    let stamp = dir.join(STAMP);
    if recently_claimed(dir) {
        return false;
    }
    if stamp.exists() {
        let _ = std::fs::remove_file(&stamp);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stamp)
        .is_ok()
}

fn last_start_failed(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(STAMP)).is_ok_and(|s| s.trim() == FAILED)
}

pub fn status_lines(resolved: &Resolved) -> Vec<String> {
    let mut out = Vec::new();
    for (name, (def, origin)) in &resolved.dependencies {
        let pipeline = pipeline_name(origin);
        let listen = def.listen.clone().unwrap_or_default();
        let state = if let Err(reason) = installable(def) {
            format!("rtk cannot install it on this computer: {reason}")
        } else {
            match state(name, def) {
                State::Missing => {
                    let busy = address_answers(&listen);
                    if busy {
                        format!(
                            "not installed; another program answers on {listen}. To install it, run `rtk init -g` in a terminal"
                        )
                    } else {
                        "not installed; to install it, run `rtk init -g` in a terminal".to_string()
                    }
                }
                State::Outdated => format!(
                    "the installed copy differs from the one {pipeline} declares; to update it, run `rtk init -g` in a terminal"
                ),
                State::Installed => {
                    let dir = dir(name).unwrap_or_default();
                    match health_of(&dir, def) {
                        Health::Ready => format!("running on {listen}"),
                        Health::Starting(detail) => format!(
                            "starting{}; until it is ready, tool calls run without the rules that need it",
                            detail.map(|d| format!(" ({d})")).unwrap_or_default()
                        ),
                        Health::Foreign => format!(
                            "another program answers on {listen}, so rtk does not start {name} there"
                        ),
                        Health::Down if last_start_failed(&dir) => format!(
                            "not running; the last start failed, see {}",
                            dir.join(LOG).display()
                        ),
                        Health::Down => {
                            "not running; rtk starts it on the next tool call that uses it"
                                .to_string()
                        }
                    }
                }
            }
        };
        out.push(format!("dependency {name} (from {pipeline}): {state}"));
    }
    for name in installed_names() {
        if !resolved.dependencies.contains_key(&name) {
            out.push(format!(
                "dependency {name}: installed, not used by the active pipelines; `rtk init -g --uninstall` removes it"
            ));
        }
    }
    out
}

fn installed_names() -> Vec<String> {
    let mut names = directory_names();
    names.retain(|name| installed(name).is_some());
    names
}

fn directory_names() -> Vec<String> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| dir(name).is_some())
        .collect();
    names.sort();
    names
}

pub fn remove_all(dry_run: bool) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for name in directory_names() {
        let Some(dir) = dir(&name) else {
            continue;
        };
        if !dry_run {
            if let Err(e) = stop(&name) {
                eprintln!(
                    "  [warn] {e:#}; its files stay in {} until it stops",
                    dir.display()
                );
                continue;
            }
            remove_inside_root(&dir)?;
        }
        removed.push(dir);
    }
    if !dry_run
        && let Some(root) = root()
        && let Ok(read) = std::fs::read_dir(&root)
    {
        for entry in read.filter_map(|e| e.ok()) {
            let leftover = entry.file_name().to_str().is_some_and(|n| {
                n.starts_with(".previous-") || (n.starts_with(".failed-") && n.ends_with(".log"))
            });
            if !leftover {
                continue;
            }
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = remove_inside_root(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    if !dry_run
        && let Some(root) = root()
        && std::fs::read_dir(&root).is_ok_and(|mut entries| entries.next().is_none())
    {
        let _ = std::fs::remove_dir(&root);
    }
    Ok(removed)
}

pub fn pipeline_name(origin: &str) -> String {
    Path::new(origin)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| origin.to_string())
}

pub fn review_of(origin: &str, entries: &[crate::core::pipeline_catalog::Entry]) -> Review {
    let name = pipeline_name(origin);
    let Some(installed) = crate::core::pipeline_remote::install_origin(&name) else {
        return Review::Unreviewed;
    };
    let (Some(sha), Some(content)) = (installed.sha, installed.content) else {
        return Review::Unreviewed;
    };
    let path = if Path::new(origin).is_file() {
        PathBuf::from(origin)
    } else {
        match crate::core::pipeline_config::user_pipelines_dir() {
            Some(dir) => dir.join(format!("{name}.toml")),
            None => return Review::Unreviewed,
        }
    };
    let unchanged = std::fs::read_to_string(&path)
        .is_ok_and(|text| crate::core::pipeline_remote::content_digest(&text) == content);
    if !unchanged {
        return Review::Unreviewed;
    }
    let reviewed = entries.iter().any(|entry| {
        entry.canonical() == installed.source && entry.sha.as_deref() == Some(sha.as_str())
    });
    if reviewed {
        Review::Reviewed(sha)
    } else {
        Review::Unreviewed
    }
}

pub fn recipe_lines(name: &str, def: &DependencyDef) -> Vec<String> {
    let clean = crate::core::utils::sanitize_for_terminal;
    let mut out = Vec::new();
    match &def.about {
        Some(about) => out.push(format!("{name}: {}", clean(about))),
        None => out.push(name.to_string()),
    }
    match &def.installer {
        Some(Installer::Npm { package, node, .. }) => {
            out.push(format!(
                "  installs: npm {} (its sha512 is checked before anything runs; npm picks the package's own dependencies)",
                clean(package)
            ));
            if let Some(major) = node {
                out.push(format!("  uses: your Node.js {major} or newer"));
            }
        }
        Some(Installer::Pip {
            requirements,
            python,
        }) => {
            let names: Vec<String> = requirements
                .iter()
                .filter_map(|r| r.split_whitespace().next().map(clean))
                .collect();
            out.push(format!(
                "  installs: pip {} (every hash is checked before anything runs)",
                names.join(", ")
            ));
            out.push(format!(
                "  uses: your Python {}",
                python
                    .as_deref()
                    .map(|v| format!("{} or newer", clean(v)))
                    .unwrap_or_else(|| "3".to_string())
            ));
        }
        Some(Installer::Binary { platforms }) => match platforms.get(&platform()) {
            Some(bin) => out.push(format!(
                "  installs: {} (its sha256 is checked before anything runs)",
                clean(&bin.url)
            )),
            None => out.push(format!("  installs: no binary for {}", platform())),
        },
        None => {}
    }
    if !def.files.is_empty() {
        let files: Vec<String> = def.files.keys().map(|f| clean(f)).collect();
        out.push(format!(
            "  writes: {} (shipped in the pipeline file)",
            files.join(", ")
        ));
    }
    let run: Vec<String> = def.run.iter().map(|part| clean(part)).collect();
    out.push(format!(
        "  runs: {} on {}",
        run.join(" "),
        clean(def.listen.as_deref().unwrap_or_default())
    ));
    if !def.env.is_empty() {
        let vars: Vec<String> = def
            .env
            .iter()
            .map(|(key, value)| format!("{}={}", clean(key), clean(value)))
            .collect();
        out.push(format!(
            "  env (only while it runs, never while installing): {}",
            vars.join(", ")
        ));
    }
    if let Some(needs) = &def.needs {
        out.push(format!("  needs: {} (as its author says)", clean(needs)));
    }
    if let Some(dir) = dir(name) {
        out.push(format!("  into: {}", dir.display()));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offered {
    Ready,
    Installed,
    Declined,
    Failed(String),
    Unavailable(String),
}

pub fn offer(
    pipeline: &str,
    name: &str,
    def: &DependencyDef,
    review: &Review,
    mode: Mode,
    refusal: &str,
) -> Result<Offered> {
    if let Err(reason) = installable(def) {
        return Ok(Offered::Unavailable(format!(
            "rtk cannot install it on this computer ({reason})"
        )));
    }
    let current = state(name, def);
    if current == State::Installed || (current == State::Missing && listen_answers(def)) {
        return Ok(Offered::Ready);
    }
    let auto = mode == Mode::Auto && matches!(review, Review::Reviewed(_));
    if mode == Mode::NoTerminal || (mode == Mode::Auto && !auto) {
        let why = if mode == Mode::Auto {
            "rtk-ai has not reviewed this pipeline, so rtk installs it only after a yes in a terminal"
        } else {
            "rtk installs it only after a yes in a terminal"
        };
        let what = if current == State::Outdated {
            format!("the installed copy differs from the one {pipeline} declares; {why}")
        } else {
            format!("not installed; {why}")
        };
        return Ok(Offered::Unavailable(what));
    }
    println!();
    let mut lines = recipe_lines(name, def);
    lines.push(String::new());
    let title = match review {
        Review::Reviewed(sha) => {
            lines.push(format!(
                "Reviewed by rtk-ai at commit {}. The software comes from its authors; rtk-ai takes no responsibility for what it does.",
                crate::core::pipeline_remote::short_sha(sha)
            ));
            format!("{pipeline} needs a local service")
        }
        Review::Unreviewed => {
            lines.push(
                "This pipeline is not reviewed by rtk-ai. You install this dependency at your own risk."
                    .to_string(),
            );
            format!("{pipeline} needs a local service (not reviewed by rtk-ai)")
        }
    };
    if auto {
        lines.push(String::new());
        lines.push(
            "Installing it without a question: you named this reviewed pipeline with --pipeline, and no terminal is here to answer. `rtk init -g --uninstall` stops and removes it."
                .to_string(),
        );
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    crate::core::pipeline_cmd::warning_panel(&title, &refs);
    if !auto {
        let accepted = crate::core::ask::confirm(
            &format!("  Install {name}?"),
            "install it and start it in the background",
            refusal,
        )?;
        if !accepted {
            return Ok(Offered::Declined);
        }
    }
    println!("  Installing {name}…");
    if let Err(e) = install(name, def) {
        return Ok(Offered::Failed(format!("the install failed: {e:#}")));
    }
    match start(name) {
        Ok(_) => {
            println!(
                "  {name}: starting in the background; `rtk pipeline status` shows when it is ready. Until then, tool calls run without the rules that need it. If it stops, rtk starts it again when a tool call needs it."
            );
            Ok(Offered::Installed)
        }
        Err(e) => Ok(Offered::Failed(format!(
            "it was installed but did not start: {e:#}"
        ))),
    }
}

pub fn find_node(min_major: u32) -> Option<(PathBuf, String)> {
    node_candidates()
        .into_iter()
        .filter(|path| path.is_file())
        .find_map(|path| {
            let version = tool_version(&path, &["--version"])?;
            (major(&version) >= min_major).then_some((path, version))
        })
}

fn node_candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    let mut out: Vec<PathBuf> = which::which_all(exe)
        .map(|found| found.collect())
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\Program Files\nodejs"));
    } else {
        dirs.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/snap/bin",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = user_dirs::home() {
            dirs.push(home.join(".volta").join("bin"));
            dirs.push(home.join(".local").join("bin"));
            dirs.extend(nvm_bins(&home.join(".nvm").join("versions").join("node")));
        }
    }
    for candidate in dirs.into_iter().map(|d| d.join(exe)) {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

fn find_python(min: Option<&str>) -> Option<PathBuf> {
    let wanted = version_key(min.unwrap_or("3"));
    let names: &[&str] = if cfg!(windows) {
        &["python.exe", "python3.exe"]
    } else {
        &["python3", "python"]
    };
    names
        .iter()
        .flat_map(|exe| {
            which::which_all(exe)
                .map(|found| found.collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .find(|path| {
            tool_version(path, &["--version"])
                .map(|v| version_key(v.trim_start_matches("Python ")))
                .is_some_and(|have| have.first() == Some(&3) && have >= wanted)
        })
}

fn nvm_bins(root: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<(Vec<u32>, PathBuf)> = read
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (version_key(&name), e.path().join("bin"))
        })
        .collect();
    versions.sort();
    versions.reverse();
    versions.into_iter().map(|(_, bin)| bin).collect()
}

fn version_key(version: &str) -> Vec<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn major(version: &str) -> u32 {
    version_key(version).first().copied().unwrap_or(0)
}

fn tool_version(tool: &Path, args: &[&str]) -> Option<String> {
    let out = command_for(tool)
        .ok()?
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let text = if out.stdout.is_empty() {
        out.stderr
    } else {
        out.stdout
    };
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&text).trim().to_string())
}

fn path_with(dirs: &[PathBuf]) -> Result<std::ffi::OsString> {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let paths = dirs.iter().cloned().chain(std::env::split_paths(&existing));
    std::env::join_paths(paths).context("build PATH for the service")
}

fn open_log(dir: &Path) -> Result<std::fs::File> {
    let path = dir.join(LOG);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))
}

#[cfg(test)]
static TEST_ROOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn test_root_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ROOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_lock() -> std::sync::MutexGuard<'static, ()> {
        test_root_lock()
    }

    const INTEGRITY: &str = "sha512-C7eBcUUj/S4Mn2ccQoqflZghEi9OPu6RBUbSjcbnuPbQQAWTVFGxDRoLyyP59hm2t4oF8JbE+dP0IaUE58XsJQ==";

    fn recipe(name: &str, run: &str) -> DependencyDef {
        let text = format!(
            "[dependency.{name}]\nabout = \"a judge\"\nnpm = \"@acme/judge@1.2.0\"\nintegrity = \"{INTEGRITY}\"\nnode = 20\nrun = [\"node\", \"{run}\"]\nlisten = \"127.0.0.1:41899\"\nhealth = \"/health\"\nenv = {{ PORT = \"41899\" }}\n[dependency.{name}.files]\n\"server.mjs\" = \"console.log(1)\"\n"
        );
        let parsed = rtk_engine::pipeline::parse(&text, "t.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let mut def = parsed.file.expect("parses").dependencies[name].clone();
        def.line = None;
        def
    }

    fn record(name: &str, def: &DependencyDef) -> PathBuf {
        let dir = dir(name).expect("dir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let marker = serde_json::json!({
            "name": name,
            "fingerprint": fingerprint(def),
            "recipe": recipe_json(def),
        });
        std::fs::write(dir.join(MARKER), marker.to_string()).expect("marker");
        dir
    }

    #[test]
    fn base64_matches_the_npm_integrity_encoding() {
        let _guard = root_lock();
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            base64(&Sha512::digest(b"abc")),
            "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw=="
        );
    }

    #[test]
    fn the_accepted_recipe_is_what_gets_started() {
        let _guard = root_lock();
        let def = recipe("roundtrip", "server.mjs");
        let stored = recipe_from_json(&recipe_json(&def)).expect("stored recipe");
        assert_eq!(stored, def);
        assert_eq!(fingerprint(&stored), fingerprint(&def));
        assert_ne!(
            fingerprint(&recipe("roundtrip", "other.mjs")),
            fingerprint(&def)
        );
    }

    #[test]
    fn state_follows_the_recorded_install() {
        let _guard = root_lock();
        let def = recipe("state-probe", "server.mjs");
        assert_eq!(state("state-probe", &def), State::Missing);
        let dir = record("state-probe", &def);
        assert_eq!(state("state-probe", &def), State::Installed);
        assert_eq!(
            state("state-probe", &recipe("state-probe", "changed.mjs")),
            State::Outdated
        );
        remove_inside_root(&dir).expect("remove");
        assert_eq!(state("state-probe", &def), State::Missing);
    }

    #[test]
    fn only_a_dependency_directory_is_ever_removed() {
        let _guard = root_lock();
        let root = root().expect("root");
        std::fs::create_dir_all(&root).expect("mkdir");
        assert!(remove_inside_root(&root).is_err());
        assert!(remove_inside_root(root.parent().expect("parent")).is_err());
        assert!(remove_inside_root(&root.join("a").join("b")).is_err());
        assert!(dir("../escape").is_none());
        assert!(dir("UPPER").is_none());
        assert!(dir("").is_none());
    }

    #[test]
    fn a_failed_install_leaves_nothing_and_uninstall_clears_leftovers() {
        let _guard = root_lock();
        let text = "[dependency.bad]\nrun = [\"x\"]\nlisten = \"127.0.0.1:41896\"\n[dependency.bad.binary.macos-arm64]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n[dependency.bad.binary.macos-x64]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n[dependency.bad.binary.linux-x64]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n[dependency.bad.binary.linux-arm64]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n[dependency.bad.binary.windows-x64]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n";
        let def = rtk_engine::pipeline::parse(text, "t.toml")
            .file
            .expect("parses")
            .dependencies["bad"]
            .clone();
        assert!(install("bad", &def).is_err());
        assert!(!dir("bad").expect("dir").exists());

        let leftover = dir("leftover").expect("dir");
        std::fs::create_dir_all(&leftover).expect("mkdir");
        std::fs::write(leftover.join(LOG), "npm failed").expect("log");
        let removed = remove_all(true).expect("dry run");
        assert!(removed.contains(&leftover));
        assert!(leftover.exists());
        remove_all(false).expect("remove");
        assert!(!leftover.exists());
    }

    fn unreachable_binary(name: &str, listen: &str, run: &str) -> DependencyDef {
        let text = format!(
            "[dependency.{name}]\nrun = [{run}]\nlisten = \"{listen}\"\n[dependency.{name}.binary.{}]\nurl = \"https://127.0.0.1:1/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n",
            platform()
        );
        let mut def = rtk_engine::pipeline::parse(&text, "t.toml")
            .file
            .expect("parses")
            .dependencies[name]
            .clone();
        def.line = None;
        def
    }

    #[test]
    fn a_failed_reinstall_puts_the_previous_copy_back() {
        let _guard = root_lock();
        let old = recipe("keep-old", "server.mjs");
        let installed_dir = record("keep-old", &old);
        std::fs::write(installed_dir.join("server.mjs"), "old").expect("write");
        let newer = unreachable_binary("keep-old", "127.0.0.1:41897", "\"x\"");
        let error = install("keep-old", &newer).expect_err("the download cannot succeed");
        assert!(format!("{error:#}").contains("was put back"), "{error:#}");
        assert_eq!(state("keep-old", &old), State::Installed);
        assert_eq!(
            std::fs::read_to_string(installed_dir.join("server.mjs")).expect("read"),
            "old"
        );
        let root = root().expect("root");
        assert!(!root.join(".previous-keep-old").exists());
        remove_inside_root(&installed_dir).expect("cleanup");
        let _ = std::fs::remove_file(root.join(".failed-keep-old.log"));
    }

    #[test]
    fn a_service_already_answering_counts_as_ready() {
        let _guard = root_lock();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let mut def = recipe("answered", "server.mjs");
        def.listen = Some(listener.local_addr().expect("addr").to_string());
        let offered = offer(
            "p",
            "answered",
            &def,
            &Review::Unreviewed,
            Mode::NoTerminal,
            "",
        )
        .expect("offer");
        assert_eq!(offered, Offered::Ready);
        assert!(!dir("answered").expect("dir").exists());
    }

    #[cfg(unix)]
    #[test]
    fn stop_leaves_alone_a_process_that_is_not_the_service() {
        let _guard = root_lock();
        let def = recipe("not-ours", "server.mjs");
        let installed_dir = record("not-ours", &def);
        std::fs::write(installed_dir.join(PID), format!("{}\n", std::process::id())).expect("pid");
        assert!(!stop("not-ours").expect("stop"));
        assert!(!installed_dir.join(PID).exists());
        remove_inside_root(&installed_dir).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn a_service_still_binding_is_starting_and_stop_ends_its_process_group() {
        let _guard = root_lock();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let def = unreachable_binary("sleeper", &format!("127.0.0.1:{port}"), "\"sleep\", \"30\"");
        let installed_dir = record("sleeper", &def);
        let mut command = crate::core::utils::resolved_command("sleep");
        command.arg("30");
        detach(&mut command);
        let mut child = command.spawn().expect("spawn sleep");
        let pid = child.id();
        std::fs::write(installed_dir.join(PID), format!("{pid}\n")).expect("pid");
        assert_eq!(health_of(&installed_dir, &def), Health::Starting(None));
        assert!(stop("sleeper").expect("stop"));
        assert!(!runs(pid, &def));
        let _ = child.wait();
        remove_inside_root(&installed_dir).expect("cleanup");
    }

    #[test]
    fn a_stamp_dated_in_the_future_does_not_block_starts() {
        let _guard = root_lock();
        let stamped = dir("future-stamp").expect("dir");
        std::fs::create_dir_all(&stamped).expect("mkdir");
        let file = std::fs::File::create(stamped.join(STAMP)).expect("stamp");
        file.set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
            .expect("set mtime");
        drop(file);
        assert!(!recently_claimed(&stamped));
        assert!(claim_start(&stamped));
        assert!(recently_claimed(&stamped));
        remove_inside_root(&stamped).expect("cleanup");
    }

    #[test]
    fn the_consent_panel_shows_the_service_environment() {
        let _guard = root_lock();
        let def = recipe("env-shown", "server.mjs");
        assert!(
            recipe_lines("env-shown", &def)
                .iter()
                .any(|l| l.contains("PORT=41899") && l.contains("only while it runs"))
        );
    }

    #[test]
    fn without_a_terminal_nothing_is_installed() {
        let _guard = root_lock();
        let def = recipe("no-terminal", "server.mjs");
        let offered = offer(
            "p",
            "no-terminal",
            &def,
            &Review::Unreviewed,
            Mode::NoTerminal,
            "",
        )
        .expect("offer");
        assert!(matches!(offered, Offered::Unavailable(_)), "{offered:?}");
        assert_eq!(state("no-terminal", &def), State::Missing);
        assert!(!dir("no-terminal").expect("dir").exists());
    }

    #[test]
    fn named_without_a_terminal_an_unreviewed_pipeline_installs_nothing() {
        let _guard = root_lock();
        let def = recipe("auto-unreviewed", "server.mjs");
        let offered = offer(
            "p",
            "auto-unreviewed",
            &def,
            &Review::Unreviewed,
            Mode::Auto,
            "",
        )
        .expect("offer");
        assert!(
            matches!(&offered, Offered::Unavailable(reason) if reason.contains("has not reviewed")),
            "{offered:?}"
        );
        assert_eq!(state("auto-unreviewed", &def), State::Missing);
        assert!(!dir("auto-unreviewed").expect("dir").exists());
    }

    #[test]
    fn a_recipe_this_computer_cannot_use_is_reported_not_installed() {
        let _guard = root_lock();
        let text = "[dependency.bin]\nrun = [\"x\"]\nlisten = \"127.0.0.1:41898\"\n[dependency.bin.binary.plan9-mips]\nurl = \"https://example.com/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n";
        let def = rtk_engine::pipeline::parse(text, "t.toml")
            .file
            .expect("parses")
            .dependencies["bin"]
            .clone();
        assert!(installable(&def).is_err_and(|r| r.contains("no binary for this computer")));
        assert!(install("bin", &def).is_err());
        let mut newer = recipe("newer", "server.mjs");
        newer.unknown = vec!["cargo".to_string()];
        assert!(installable(&newer).is_err_and(|r| r.contains("cargo")));
    }

    #[test]
    fn only_a_linked_installed_dependency_is_woken() {
        let _guard = root_lock();
        let hosted = UpstreamDef {
            provider: "jev".to_string(),
            base_url: Some("https://api.example.com/v1/systemone".to_string()),
            ..Default::default()
        };
        wake(&hosted);
        let missing = UpstreamDef {
            dependency: Some("never-installed".to_string()),
            base_url: Some("http://127.0.0.1:41897/v1/systemone".to_string()),
            ..Default::default()
        };
        wake(&missing);
        assert!(!dir("never-installed").expect("dir").exists());
    }

    #[test]
    fn a_start_is_tried_once_then_waits() {
        let _guard = root_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(claim_start(tmp.path()));
        assert!(!claim_start(tmp.path()));
        let stamp = std::fs::File::options()
            .write(true)
            .open(tmp.path().join(STAMP))
            .expect("stamp");
        stamp
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .expect("age");
        assert!(claim_start(tmp.path()));
        std::fs::write(tmp.path().join(STAMP), FAILED).expect("fail");
        let stamp = std::fs::File::options()
            .write(true)
            .open(tmp.path().join(STAMP))
            .expect("stamp");
        stamp
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .expect("age");
        assert!(!claim_start(tmp.path()));
    }

    #[test]
    fn health_answers_and_addresses_parse() {
        let _guard = root_lock();
        assert_eq!(
            parse_response("HTTP/1.1 503 Service Unavailable\r\n\r\ndownloading model.onnx 42%"),
            Some((503, "downloading model.onnx 42%".to_string()))
        );
        assert_eq!(parse_response("garbage"), None);
        assert_eq!(
            short_line("downloading model.onnx 42%").as_deref(),
            Some("downloading model.onnx 42%")
        );
        assert_eq!(short_line("{\"error\":\"loading\"}"), None);
        assert_eq!(short_line(""), None);
        assert_eq!(
            socket_addr("127.0.0.1:41733"),
            Some(SocketAddr::from(([127, 0, 0, 1], 41733)))
        );
        assert!(socket_addr("[::1]:9000").is_some());
        assert_eq!(socket_addr("10.0.0.1:41733"), None);
    }

    #[test]
    fn a_catalog_pin_is_reviewed_any_other_is_not() {
        let _guard = root_lock();
        let sha = "26b8683d0eeffde220a8d25a60b9654f752c7eb4";
        let reviewed = "schema_version = 1\n[pipeline]\nname = \"review-probe\"\n";
        let dir = crate::core::pipeline_config::user_pipelines_dir().expect("pipelines dir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("review-probe.toml");
        std::fs::write(&file, reviewed).expect("write");
        let origin = file.display().to_string();
        crate::core::pipeline_remote::record_install(
            "review-probe",
            "github:acme/rtk-pipeline-probe",
            Some(sha),
            reviewed,
        );
        let entry = |sha: &str| crate::core::pipeline_catalog::Entry {
            id: "acme/rtk-pipeline-probe".to_string(),
            name: "review-probe".to_string(),
            summary: String::new(),
            result: None,
            requires: Vec::new(),
            sha: Some(sha.to_string()),
            install: format!("rtk pipeline add github:acme/rtk-pipeline-probe@{sha}"),
        };
        assert_eq!(
            review_of(&origin, &[entry(sha)]),
            Review::Reviewed(sha.to_string())
        );
        assert_eq!(
            review_of("review-probe", &[entry(sha)]),
            Review::Reviewed(sha.to_string())
        );
        assert_eq!(
            review_of(
                &origin,
                &[entry("0000000000000000000000000000000000000000")]
            ),
            Review::Unreviewed
        );
        assert_eq!(
            review_of("/x/never-added.toml", &[entry(sha)]),
            Review::Unreviewed
        );
        std::fs::write(&file, format!("{reviewed}# edited\n")).expect("edit");
        assert_eq!(review_of(&origin, &[entry(sha)]), Review::Unreviewed);
        std::fs::write(&file, reviewed).expect("restore");
        crate::core::pipeline_remote::forget_install("review-probe");
        assert_eq!(review_of(&origin, &[entry(sha)]), Review::Unreviewed);
    }
}
