//! Preserve Claude plugin Bash hooks when RTK owns the Bash matcher.
//!
//! Claude Code can discard one hook's updatedInput when several plugin
//! entries match Bash. rtk init --global therefore removes Bash from
//! compatible plugin-cache entries and dispatches their commands from the RTK
//! hook. The manifest records exact pre/post arrays so uninstall can restore a
//! file only when no plugin changed it after installation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const MANIFEST_FILE: &str = "rtk-bash-manifest.json";
const FALLTHROUGH_GUARD: &str = "RTK_MANIFEST_FALLTHROUGH";
const CLAUDE_DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(10);
const HOOK_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
struct Manifest {
    #[serde(default)]
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ManifestEntry {
    cache_path: String,
    original_entries: Vec<Value>,
    patched_entries: Vec<Value>,
    commands: Vec<ManifestCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_data: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
enum ManifestCommand {
    /// Claude executes a command without `args` through the platform shell.
    Shell(String),
    /// Claude executes a command with the supplied arguments as exact argv.
    Exec(ExecCommand),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct ExecCommand {
    command: String,
    args: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ManifestResult {
    NoBlock,
    Blocked { stdout: String, stderr: Vec<u8> },
}

fn manifest_path(claude_dir: &Path) -> PathBuf {
    claude_dir.join("hooks").join(MANIFEST_FILE)
}

fn read_manifest(path: &Path) -> Result<Option<Manifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read manifest {}", path.display()))?;
    let manifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse manifest {}", path.display()))?;
    Ok(Some(manifest))
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("Manifest has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create manifest directory {}", parent.display()))?;
    let content = serde_json::to_string_pretty(manifest).context("Failed to serialize manifest")?;
    super::init::atomic_write(path, &content)
}

pub(crate) fn is_json_deny(output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(output.trim()) else {
        return false;
    };
    value
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
        == Some("deny")
        || value.get("decision").and_then(Value::as_str) == Some("deny")
}

pub(crate) fn deny_reason(output: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(output.trim()).ok()?;
    value
        .pointer("/hookSpecificOutput/permissionDecisionReason")
        .and_then(Value::as_str)
        .or_else(|| value.get("reason").and_then(Value::as_str))
        .map(str::to_owned)
}

fn wait_for_output(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<Option<std::process::Output>> {
    let stdout = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            pipe.read_to_end(&mut output).map(|_| output)
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            pipe.read_to_end(&mut output).map(|_| output)
        })
    });
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().context("Failed to poll plugin hook")? {
            let stdout = join_pipe(stdout).context("Failed to capture plugin hook stdout")?;
            let stderr = join_pipe(stderr).context("Failed to capture plugin hook stderr")?;
            return Ok(Some(std::process::Output {
                status,
                stdout,
                stderr,
            }));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // A timed-out hook may have left descendants holding the pipes open. Dropping
            // these handles avoids waiting for those descendants while their readers finish
            // naturally when the inherited descriptors close.
            return Ok(None);
        }
        std::thread::sleep(HOOK_POLL_INTERVAL);
    }
}

fn join_pipe(pipe: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>) -> Result<Vec<u8>> {
    pipe.map(|pipe| {
        pipe.join()
            .map_err(|_| anyhow::anyhow!("plugin hook output reader panicked"))?
            .context("failed to read plugin hook output")
    })
    .transpose()
    .map(|output| output.unwrap_or_default())
}

fn write_stdin_in_background(child: &mut std::process::Child, payload: &str) -> Arc<AtomicBool> {
    let write_ok = Arc::new(AtomicBool::new(false));
    let Some(mut stdin) = child.stdin.take() else {
        return write_ok;
    };
    let payload = payload.as_bytes().to_owned();
    let completed = Arc::clone(&write_ok);
    std::thread::spawn(move || {
        completed.store(stdin.write_all(&payload).is_ok(), Ordering::Release);
    });
    write_ok
}

pub(crate) fn run_manifest_handlers(claude_dir: &Path, payload: &str) -> ManifestResult {
    if std::env::var_os(FALLTHROUGH_GUARD).is_some() {
        return ManifestResult::NoBlock;
    }

    let Some(manifest) = read_manifest(&manifest_path(claude_dir)).ok().flatten() else {
        return ManifestResult::NoBlock;
    };

    let mut first_block: Option<String> = None;
    let mut stderr = Vec::new();

    for entry in manifest.entries {
        if !Path::new(&entry.cache_path).exists() || !entry_is_active(&entry) {
            continue;
        }
        for handler in entry.commands {
            if let Some(plugin_data) = entry.plugin_data.as_deref() {
                let _ = fs::create_dir_all(plugin_data);
            }
            let mut command = handler.process();
            command.env(FALLTHROUGH_GUARD, "1");
            if let Some(plugin_root) = entry.plugin_root.as_deref() {
                command
                    .env("CLAUDE_PLUGIN_ROOT", plugin_root)
                    .env("PLUGIN_ROOT", plugin_root);
            }
            if let Some(plugin_data) = entry.plugin_data.as_deref() {
                command
                    .env("CLAUDE_PLUGIN_DATA", plugin_data)
                    .env("PLUGIN_DATA", plugin_data);
            }
            let mut child = match command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(_) => continue,
            };

            let write_ok = write_stdin_in_background(&mut child, payload);
            let Ok(Some(output)) = wait_for_output(child, CLAUDE_DEFAULT_HOOK_TIMEOUT) else {
                continue;
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let blocked = (output.status.code() == Some(2) && write_ok.load(Ordering::Acquire))
                || is_json_deny(&stdout);
            if blocked && first_block.is_none() {
                first_block = Some(stdout.into_owned());
                stderr.extend_from_slice(&output.stderr);
            }
        }
    }

    first_block.map_or(ManifestResult::NoBlock, |stdout| ManifestResult::Blocked {
        stdout,
        stderr,
    })
}

fn command_for_program(program: &str) -> Command {
    match crate::core::utils::resolve_binary(program) {
        // This executable comes from an existing Claude plugin hook. Replaying its
        // exact argv is the compatibility contract of manifest fallthrough.
        // nosemgrep: dynamic-command-execution -- trusted Claude plugin command replay
        Ok(path) => Command::new(path),
        // nosemgrep: dynamic-command-execution -- trusted Claude plugin command replay
        Err(_) => Command::new(program),
    }
}

fn build_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    }
    #[cfg(not(windows))]
    {
        // Claude's shell-form hook is already an explicitly configured command;
        // this preserves its POSIX shell semantics during fallthrough.
        // nosemgrep: interpreter-execution -- replay configured Claude shell hook
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    }
}

impl ManifestCommand {
    fn process(&self) -> Command {
        match self {
            Self::Shell(command) => build_shell_command(command),
            Self::Exec(command) => {
                let mut process = command_for_program(&command.command);
                process.args(&command.args);
                process
            }
        }
    }
}

fn matcher_contains_bash(matcher: &str) -> bool {
    matcher.split('|').any(|part| part.trim() == "Bash")
}

fn remove_bash(matcher: &str) -> String {
    matcher
        .split('|')
        .filter(|part| part.trim() != "Bash")
        .collect::<Vec<_>>()
        .join("|")
}

fn parse_version(path: &Path) -> Option<(u32, u32, u32)> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .strip_prefix('v')
        .unwrap_or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
        });
    let parts: Vec<u32> = name
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    Some((
        parts.first().copied().unwrap_or_default(),
        parts.get(1).copied().unwrap_or_default(),
        parts.get(2).copied().unwrap_or_default(),
    ))
}

fn entry_is_active(entry: &ManifestEntry) -> bool {
    let cache_path = Path::new(&entry.cache_path);
    let Some(version_dir) = cache_path.parent().and_then(Path::parent) else {
        return true;
    };
    if parse_version(version_dir).is_none() {
        return true;
    }
    let Some(plugin_dir) = version_dir.parent() else {
        return true;
    };
    active_version(plugin_dir)
        .as_deref()
        .is_none_or(|active| active == version_dir)
}

fn resolve_plugin_placeholders(command: &str, plugin_root: &Path, plugin_data: &Path) -> String {
    let root = plugin_root.to_string_lossy();
    let data = plugin_data.to_string_lossy();
    command
        .replace("${CLAUDE_PLUGIN_ROOT}", &root)
        .replace("${PLUGIN_ROOT}", &root)
        .replace("${CLAUDE_PLUGIN_DATA}", &data)
        .replace("${PLUGIN_DATA}", &data)
}

fn plugin_data_path(claude_dir: &Path, vendor: &str, plugin: &str) -> PathBuf {
    let id = format!("{plugin}@{vendor}");
    let id = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    claude_dir.join("plugins").join("data").join(id)
}

fn resolved_commands(
    entry: &Value,
    plugin_root: &Path,
    plugin_data: &Path,
) -> Option<Vec<ManifestCommand>> {
    let hooks = entry.get("hooks")?.as_array()?;
    let mut commands = Vec::new();
    for hook in hooks {
        // Leave hooks with execution semantics that RTK cannot reproduce in
        // the plugin cache so Claude remains their sole dispatcher.
        if hook
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("command"))
            || hook.get("timeout").is_some()
            || hook.get("shell").is_some()
            || hook
                .get("async")
                .is_some_and(|value| value.as_bool() != Some(false))
            || hook
                .get("asyncRewake")
                .is_some_and(|value| value.as_bool() != Some(false))
        {
            return None;
        }
        let command =
            resolve_plugin_placeholders(hook.get("command")?.as_str()?, plugin_root, plugin_data);
        let handler = match hook.get("args") {
            Some(args) => ManifestCommand::Exec(ExecCommand {
                command,
                args: args
                    .as_array()?
                    .iter()
                    .map(|arg| {
                        arg.as_str()
                            .map(|arg| resolve_plugin_placeholders(arg, plugin_root, plugin_data))
                    })
                    .collect::<Option<Vec<_>>>()?,
            }),
            None => ManifestCommand::Shell(command),
        };
        commands.push(handler);
    }
    (!commands.is_empty()).then_some(commands)
}

fn active_version(plugin_dir: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = fs::read_dir(plugin_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort_by_key(|path| {
        (
            parse_version(path).is_some(),
            parse_version(path),
            path.clone(),
        )
    });
    versions.pop()
}

fn patch_hook_file(
    path: &Path,
    vendor: &str,
    plugin: &str,
    claude_dir: &Path,
) -> Result<Option<ManifestEntry>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read plugin hook {}", path.display()))?;
    let mut json: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse plugin hook {}", path.display()))?;
    let Some(entries) = json
        .get_mut("hooks")
        .and_then(|hooks| hooks.get_mut("PreToolUse"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(None);
    };

    let original_entries = entries.clone();
    let mut patched_entries = Vec::with_capacity(entries.len());
    let mut commands = Vec::new();
    let mut changed = false;
    let Some(plugin_root) = path.parent().and_then(Path::parent).map(Path::to_path_buf) else {
        return Ok(None);
    };
    let plugin_data = plugin_data_path(claude_dir, vendor, plugin);

    for entry in &original_entries {
        let matcher = entry
            .get("matcher")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matcher_contains_bash(matcher) {
            patched_entries.push(entry.clone());
            continue;
        }
        let Some(entry_commands) = resolved_commands(entry, &plugin_root, &plugin_data) else {
            return Ok(None);
        };
        commands.extend(entry_commands);
        let matcher_without_bash = remove_bash(matcher);
        if matcher_without_bash.is_empty() {
            changed = true;
            continue;
        }
        let mut patched = entry.clone();
        patched["matcher"] = Value::String(matcher_without_bash);
        changed |= patched != *entry;
        patched_entries.push(patched);
    }

    if !changed {
        return Ok(None);
    }
    *entries = patched_entries.clone();
    let serialized =
        serde_json::to_string_pretty(&json).context("Failed to serialize plugin hook")?;
    super::init::atomic_write(path, &serialized)?;
    Ok(Some(ManifestEntry {
        cache_path: path.to_string_lossy().into_owned(),
        original_entries,
        patched_entries,
        commands,
        plugin_root: Some(plugin_root.to_string_lossy().into_owned()),
        plugin_data: Some(plugin_data.to_string_lossy().into_owned()),
    }))
}

pub(crate) fn patch_plugin_caches(claude_dir: &Path, verbose: u8) -> Result<usize> {
    let cache_root = claude_dir.join("plugins").join("cache");
    if !cache_root.exists() {
        return Ok(0);
    }
    let path = manifest_path(claude_dir);
    let mut manifest = read_manifest(&path)?.unwrap_or_default();
    let mut patched_count = 0;

    let Ok(vendors) = fs::read_dir(&cache_root) else {
        return Ok(0);
    };
    for vendor_entry in vendors.flatten().filter(|entry| entry.path().is_dir()) {
        let vendor_path = vendor_entry.path();
        let vendor = vendor_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let Ok(plugins) = fs::read_dir(&vendor_path) else {
            continue;
        };
        for plugin_entry in plugins.flatten().filter(|entry| entry.path().is_dir()) {
            let plugin_path = plugin_entry.path();
            let plugin = plugin_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let Some(version_path) = active_version(&plugin_path) else {
                continue;
            };
            let hooks_path = version_path.join("hooks");
            let Ok(hook_files) = fs::read_dir(hooks_path) else {
                continue;
            };
            for hook_file in hook_files.flatten() {
                let hook_path = hook_file.path();
                if hook_path.extension().and_then(|ext| ext.to_str()) != Some("json")
                    || manifest
                        .entries
                        .iter()
                        .any(|entry| entry.cache_path == hook_path.to_string_lossy())
                {
                    continue;
                }
                match patch_hook_file(&hook_path, vendor, plugin, claude_dir) {
                    Ok(Some(entry)) => {
                        manifest.entries.push(entry);
                        patched_count += 1;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if verbose > 0 {
                            eprintln!(
                                "Warning: skipped plugin hook {}: {error}",
                                hook_path.display()
                            );
                        }
                    }
                }
            }
        }
    }

    if manifest.entries.is_empty() {
        if path.exists() {
            // nosemgrep: filesystem-deletion -- remove only RTK's empty manifest
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove empty manifest {}", path.display()))?;
        }
    } else {
        write_manifest(&path, &manifest)?;
    }
    Ok(patched_count)
}

pub(crate) fn restore_plugin_caches(
    claude_dir: &Path,
    dry_run: bool,
    verbose: u8,
) -> Result<usize> {
    let path = manifest_path(claude_dir);
    let Some(manifest) = read_manifest(&path)? else {
        return Ok(0);
    };
    let mut remaining = Vec::new();
    let mut restored = 0;

    for entry in manifest.entries {
        let cache_path = Path::new(&entry.cache_path);
        if !cache_path.exists() {
            restored += 1;
            continue;
        }
        let content = fs::read_to_string(cache_path)
            .with_context(|| format!("Failed to read plugin hook {}", cache_path.display()))?;
        let mut json: Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse plugin hook {}", cache_path.display()))?;
        let Some(current) = json
            .get("hooks")
            .and_then(|hooks| hooks.get("PreToolUse"))
            .and_then(Value::as_array)
        else {
            remaining.push(entry);
            continue;
        };
        if current != entry.patched_entries.as_slice() {
            if verbose > 0 {
                eprintln!(
                    "Skipped restore for changed plugin hook {}",
                    cache_path.display()
                );
            }
            remaining.push(entry);
            continue;
        }
        restored += 1;
        if !dry_run {
            json["hooks"]["PreToolUse"] = Value::Array(entry.original_entries);
            let serialized = serde_json::to_string_pretty(&json)
                .context("Failed to serialize restored plugin hook")?;
            super::init::atomic_write(cache_path, &serialized)?;
        }
    }

    if !dry_run {
        if remaining.is_empty() {
            // nosemgrep: filesystem-deletion -- remove only RTK's consumed manifest
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove manifest {}", path.display()))?;
        } else {
            write_manifest(&path, &Manifest { entries: remaining })?;
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn write_json(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn hook_file(root: &Path) -> PathBuf {
        root.join("plugins/cache/vendor/plugin/1.0.0/hooks/hooks.json")
    }

    fn cache_fixture() -> Value {
        json!({
            "hooks": {"PreToolUse": [
                {"matcher": "Write|Bash|Edit", "hooks": [
                    {"type": "command", "command": "/bin/echo plugin"},
                    {"type": "command", "command": "/bin/printf 'audit'"}
                ]},
                {"matcher": "Bash", "hooks": [
                    {"type": "command", "command": "/bin/echo bash-only"}
                ]}
            ]}
        })
    }

    fn cache_with_hook(hook: Value) -> Value {
        json!({
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [hook]}]}
        })
    }

    fn assert_patch_skipped(value: Value) {
        let tmp = TempDir::new().unwrap();
        let path = hook_file(tmp.path());
        write_json(&path, &value);
        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 0);
        let after: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(after, value);
    }

    #[test]
    fn matcher_removal_keeps_non_bash_tokens_and_rejects_substrings() {
        assert!(matcher_contains_bash("Write | Bash | Edit"));
        assert!(!matcher_contains_bash("BashPipeline|Edit"));
        assert_eq!(remove_bash("Write|Bash|Edit"), "Write|Edit");
        assert_eq!(remove_bash("Bash"), "");
    }

    #[test]
    fn resolves_plugin_root_and_data_placeholders() {
        let commands = resolved_commands(
            &json!({"hooks": [{"command": "${CLAUDE_PLUGIN_ROOT}/run ${CLAUDE_PLUGIN_DATA}"}]}),
            Path::new("/plugins/cache/vendor/plugin/1.0.0"),
            Path::new("/plugins/data/plugin-vendor"),
        );
        assert_eq!(
            commands,
            Some(vec![ManifestCommand::Shell(
                "/plugins/cache/vendor/plugin/1.0.0/run /plugins/data/plugin-vendor".to_string(),
            )])
        );
        assert_eq!(
            plugin_data_path(Path::new("/home/user/.claude"), "vendor", "plugin"),
            PathBuf::from("/home/user/.claude/plugins/data/plugin-vendor")
        );
    }

    #[test]
    fn patch_is_idempotent_and_restore_is_conditional() {
        let tmp = TempDir::new().unwrap();
        let path = hook_file(tmp.path());
        let original = cache_fixture();
        write_json(&path, &original);

        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 1);
        let patched: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(patched["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(patched["hooks"]["PreToolUse"][0]["matcher"], "Write|Edit");
        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 0);

        assert_eq!(restore_plugin_caches(tmp.path(), false, 0).unwrap(), 1);
        let restored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored, original);
        assert!(!manifest_path(tmp.path()).exists());
    }

    #[test]
    fn restore_leaves_plugin_changes_untouched() {
        let tmp = TempDir::new().unwrap();
        let path = hook_file(tmp.path());
        write_json(&path, &cache_fixture());
        patch_plugin_caches(tmp.path(), 0).unwrap();

        let mut changed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        changed["hooks"]["PreToolUse"][0]["matcher"] = json!("Write|Edit|Bash");
        write_json(&path, &changed);
        assert_eq!(restore_plugin_caches(tmp.path(), false, 0).unwrap(), 0);
        assert!(manifest_path(tmp.path()).exists());
    }

    #[test]
    fn plugin_upgrade_keeps_old_snapshot_for_uninstall() {
        let tmp = TempDir::new().unwrap();
        let plugin_root = tmp.path().join("plugins/cache/vendor/plugin");
        let old = plugin_root.join("1.0.0/hooks/hooks.json");
        let new = plugin_root.join("2.0.0/hooks/hooks.json");
        let original = json!({"hooks": {"PreToolUse": [{
            "matcher": "Bash|Edit",
            "hooks": [{"type": "command", "command": "/bin/echo hook"}]
        }]}});

        write_json(&old, &original);
        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 1);
        write_json(&new, &original);
        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 1);

        let manifest: Manifest =
            serde_json::from_str(&fs::read_to_string(manifest_path(tmp.path())).unwrap()).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(restore_plugin_caches(tmp.path(), false, 0).unwrap(), 2);
        assert_eq!(
            fs::read_to_string(old).unwrap(),
            fs::read_to_string(new).unwrap()
        );
    }

    #[test]
    fn unsupported_execution_metadata_does_not_patch_cache_file() {
        for hook in [
            json!({"type": "command", "command": "/bin/echo ok", "timeout": 5}),
            json!({"type": "command", "command": "/bin/echo ok", "async": true}),
            json!({"type": "command", "command": "/bin/echo ok", "shell": "bash"}),
        ] {
            assert_patch_skipped(cache_with_hook(hook));
        }
    }

    #[test]
    fn manifest_preserves_exact_arguments_for_exec_form_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = hook_file(tmp.path());
        #[cfg(unix)]
        let exec_hook = json!({
            "type": "command",
            "command": "/bin/printf",
            "args": ["%s", "hello world"]
        });
        #[cfg(windows)]
        let exec_hook = json!({
            "type": "command",
            "command": "cmd.exe",
            "args": ["/C", "echo hello world"]
        });
        write_json(&path, &cache_with_hook(exec_hook));

        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 1);
        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(manifest_path(tmp.path())).unwrap()).unwrap();
        #[cfg(unix)]
        let expected_exec = json!({"command": "/bin/printf", "args": ["%s", "hello world"]});
        #[cfg(windows)]
        let expected_exec = json!({"command": "cmd.exe", "args": ["/C", "echo hello world"]});
        assert_eq!(
            manifest["entries"][0]["commands"][0], expected_exec,
            "exec-form hook arguments must be preserved"
        );

        let direct_tmp = TempDir::new().unwrap();
        let direct_path = hook_file(direct_tmp.path());
        #[cfg(unix)]
        let direct = cache_with_hook(json!({
            "type": "command",
            "command": "/bin/sh",
            "args": [
                "-c",
                "test \"$1\" = \"hello world\" && exit 2",
                "handler",
                "hello world"
            ]
        }));
        #[cfg(windows)]
        let direct = cache_with_hook(json!({
            "type": "command",
            "command": "cmd.exe",
            "args": ["/C", "exit /b 2", "handler", "hello world"]
        }));
        write_json(&direct_path, &direct);
        assert_eq!(patch_plugin_caches(direct_tmp.path(), 0).unwrap(), 1);
        assert_eq!(
            run_manifest_handlers(direct_tmp.path(), "{\"tool_name\":\"Bash\"}"),
            ManifestResult::Blocked {
                stdout: String::new(),
                stderr: Vec::new(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_form_hook_keeps_shell_pipeline_behavior() {
        let tmp = TempDir::new().unwrap();
        let path = hook_file(tmp.path());
        let value = cache_with_hook(json!({
            "type": "command",
            "command": "test \"$RTK_MANIFEST_FALLTHROUGH\" = 1 && printf '{\"decision\":\"deny\",\"reason\":\"shell-form\"}' | cat"
        }));
        write_json(&path, &value);

        assert_eq!(patch_plugin_caches(tmp.path(), 0).unwrap(), 1);
        assert_eq!(
            run_manifest_handlers(tmp.path(), "{\"tool_name\":\"Bash\"}"),
            ManifestResult::Blocked {
                stdout: "{\"decision\":\"deny\",\"reason\":\"shell-form\"}".into(),
                stderr: Vec::new(),
            }
        );
    }

    #[test]
    fn deny_detection_accepts_claude_and_gemini_shapes() {
        assert!(is_json_deny(
            r#"{"hookSpecificOutput":{"permissionDecision":"deny"}}"#
        ));
        assert!(is_json_deny(r#"{"decision":"deny","reason":"blocked"}"#));
        assert_eq!(
            deny_reason(r#"{"decision":"deny","reason":"blocked"}"#),
            Some("blocked".into())
        );
        assert!(!is_json_deny("not json"));
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_handler_is_killed_without_blocking_dispatch() {
        // nosemgrep: interpreter-execution -- test-only timeout fixture
        let child = Command::new("sh")
            .args(["-c", "sleep 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();

        assert!(wait_for_output(child, Duration::from_millis(25))
            .unwrap()
            .is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[cfg(unix)]
    #[test]
    fn non_reading_handler_cannot_block_before_timeout() {
        // nosemgrep: interpreter-execution -- test-only pipe-backpressure fixture
        let mut child = Command::new("sh")
            .args(["-c", "sleep 1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();

        write_stdin_in_background(&mut child, &"x".repeat(1_048_576));
        assert!(wait_for_output(child, Duration::from_millis(25))
            .unwrap()
            .is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[cfg(unix)]
    #[test]
    fn completed_handler_with_large_output_is_not_killed_for_pipe_backpressure() {
        // nosemgrep: interpreter-execution -- test-only pipe-drain fixture
        let child = Command::new("sh")
            .args(["-c", "yes x | head -n 100000"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();

        let output = wait_for_output(child, Duration::from_secs(2))
            .unwrap()
            .expect("large output must be drained while the child runs");
        assert!(output.stdout.len() > 100_000);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_dispatch_returns_handler_deny_without_writing_protocol_output() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let cache = hook_file(tmp.path());
        write_json(&cache, &json!({}));
        let handler = tmp.path().join("handler");
        fs::write(
            &handler,
            "#!/bin/sh\nprintf '{\"decision\":\"deny\",\"reason\":\"%s\"}' \"$CLAUDE_PLUGIN_ROOT\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&handler).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&handler, permissions).unwrap();
        write_manifest(
            &manifest_path(tmp.path()),
            &Manifest {
                entries: vec![ManifestEntry {
                    cache_path: cache.to_string_lossy().into_owned(),
                    original_entries: Vec::new(),
                    patched_entries: Vec::new(),
                    commands: vec![ManifestCommand::Shell(
                        handler.to_string_lossy().into_owned(),
                    )],
                    plugin_root: Some("/plugin/root".into()),
                    plugin_data: Some(tmp.path().join("data").to_string_lossy().into_owned()),
                }],
            },
        )
        .unwrap();

        assert_eq!(
            run_manifest_handlers(tmp.path(), "{\"tool_name\":\"Bash\"}"),
            ManifestResult::Blocked {
                stdout: "{\"decision\":\"deny\",\"reason\":\"/plugin/root\"}".into(),
                stderr: Vec::new(),
            }
        );
        assert!(tmp.path().join("data").is_dir());
    }
}
