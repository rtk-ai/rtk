use crate::block::Block;
use crate::method::{Method, MethodCtx, MethodInfo, Outcome};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_CONCURRENT: usize = 4;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

static BINARIES: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();

struct Gate {
    active: Mutex<usize>,
    freed: Condvar,
}

static GATE: OnceLock<Gate> = OnceLock::new();

fn gate() -> &'static Gate {
    GATE.get_or_init(|| Gate {
        active: Mutex::new(0),
        freed: Condvar::new(),
    })
}

struct GateGuard;

impl GateGuard {
    fn acquire() -> Option<GateGuard> {
        let gate = gate();
        let mut active = gate.active.lock().ok()?;
        while *active >= MAX_CONCURRENT {
            active = gate.freed.wait(active).ok()?;
        }
        *active += 1;
        Some(GateGuard)
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        let gate = gate();
        if let Ok(mut active) = gate.active.lock() {
            *active = active.saturating_sub(1);
            gate.freed.notify_one();
        }
    }
}

pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    let cache = BINARIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    if let Some(cached) = cache.get(name) {
        return cached.clone();
    }
    let resolved = resolve_uncached(name);
    cache.insert(name.to_string(), resolved.clone());
    resolved
}

/// Windows needs an extension before a bare name names a file: `cmd` is not
/// on disk, `cmd.exe` is. PATHEXT holds the list the shell would try, in
/// order; a name that already carries an extension is taken as given.
///
/// Split out from the platform gate so it is exercised on every host, not
/// only on the one where it matters.
#[cfg_attr(not(windows), allow(dead_code))]
fn names_with_extensions(name: &str, pathext: &str) -> Vec<String> {
    if std::path::Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    pathext
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .map(|ext| format!("{name}{ext}"))
        .collect()
}

#[cfg(windows)]
fn candidate_names(name: &str) -> Vec<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    names_with_extensions(name, &pathext)
}

#[cfg(not(windows))]
fn candidate_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

fn resolve_uncached(name: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(name);
    if candidate.is_absolute() {
        return candidate.is_file().then_some(candidate);
    }
    let path = std::env::var_os("PATH")?;
    let names = candidate_names(name);
    std::env::split_paths(&path)
        .find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
}

pub fn run(binary: &str, args: &[&str], input: &str, timeout: Duration) -> Option<String> {
    let resolved = resolve_binary(binary)?;
    let _slot = GateGuard::acquire()?;

    let mut child = Command::new(resolved)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take()?;
    let input_owned = input.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input_owned.as_bytes());
    });

    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok().map(|_| out)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = writer.join();
                    drop(reader);
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };

    let _ = writer.join();
    let output = reader.join().ok().flatten()?;
    status.success().then_some(output)
}

pub struct ProcessMethod {
    pub method_name: &'static str,
    pub binary: &'static str,
    pub args: &'static [&'static str],
    pub timeout: Duration,
    pub method_info: MethodInfo,
}

impl Method for ProcessMethod {
    fn name(&self) -> &'static str {
        self.method_name
    }

    fn info(&self) -> MethodInfo {
        self.method_info
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        match run(self.binary, self.args, block.text, self.timeout) {
            Some(output) => Outcome::Continue(output),
            None => Outcome::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ContentFormat;
    use crate::method::CostClass;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path.display().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn stdin_becomes_stdout_on_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = script(dir.path(), "upper.sh", "tr '[:lower:]' '[:upper:]'");
        let out = run(&bin, &[], "hello", Duration::from_secs(5)).expect("runs");
        assert_eq!(out, "HELLO");
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_missing_binary_and_timeout_all_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fails = script(dir.path(), "fails.sh", "exit 3");
        assert!(run(&fails, &[], "x", Duration::from_secs(5)).is_none());

        assert!(run("/definitely/not/a/binary", &[], "x", Duration::from_secs(1)).is_none());

        let hangs = script(
            dir.path(),
            "hangs.sh",
            "cat >/dev/null; sleep 30; echo done",
        );
        let started = Instant::now();
        assert!(run(&hangs, &[], "x", Duration::from_millis(200)).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "killed, not waited out"
        );
    }

    #[test]
    fn process_method_is_fail_open_through_the_trait() {
        let method = ProcessMethod {
            method_name: "test-proc",
            binary: "/definitely/not/a/binary",
            args: &[],
            timeout: Duration::from_millis(200),
            method_info: MethodInfo {
                lossy: true,
                idempotent: false,
                cost: CostClass::Fast,
                ..MethodInfo::LOSSLESS_TRANSFORM
            },
        };
        let ctx = MethodCtx::new("test");
        let outcome = method.apply(&crate::testutil::block("body", ContentFormat::Term), &ctx);
        assert_eq!(outcome, Outcome::Skip);
    }

    #[test]
    fn path_resolution_is_cached_and_absolute_paths_pass_through() {
        #[cfg(unix)]
        let (on_path, absolute) = ("sh", "/bin/sh".to_string());
        #[cfg(windows)]
        let (on_path, absolute) = ("cmd", std::env::var("COMSPEC").expect("COMSPEC is set"));

        assert!(resolve_binary(on_path).is_some());
        assert!(resolve_binary(on_path).is_some());
        assert!(resolve_binary(&absolute).is_some());
        assert!(resolve_binary("no-such-binary-xyz").is_none());
    }

    #[test]
    fn windows_names_get_every_pathext_extension_in_order() {
        assert_eq!(
            names_with_extensions("cmd", ".COM;.EXE;.BAT"),
            vec!["cmd.COM", "cmd.EXE", "cmd.BAT"]
        );
    }

    #[test]
    fn an_explicit_extension_is_taken_as_given() {
        assert_eq!(
            names_with_extensions("ripgrep.exe", ".COM;.EXE"),
            vec!["ripgrep.exe"]
        );
    }

    #[test]
    fn empty_pathext_entries_are_skipped() {
        assert_eq!(names_with_extensions("cmd", ".EXE;;"), vec!["cmd.EXE"]);
    }
}
