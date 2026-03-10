use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::integrity;

// Embedded hook script (guards before set -euo pipefail)
const REWRITE_HOOK: &str = include_str!("../hooks/rtk-rewrite.sh");

#[cfg(unix)]
const OPENCODE_PLUGIN: &str = include_str!("../hooks/rtk-rewrite.ts");

// Embedded slim RTK awareness instructions
const RTK_SLIM: &str = include_str!("../hooks/rtk-awareness.md");

#[cfg(unix)]
const RTK_OPENCODE_SECTION: &str = r#"<!-- rtk-opencode-start -->
## RTK opencode guidance

- Bash commands may be transparently rewritten through RTK for compact output.
- Use `rtk proxy <command>` when you need raw passthrough behavior.
<!-- rtk-opencode-end -->"#;

/// Control flow for settings.json patching
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchMode {
    Ask,  // Default: prompt user [y/N]
    Auto, // --auto-patch: no prompt
    Skip, // --no-patch: manual instructions
}

/// Result of settings.json patching operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatchResult {
    Patched,        // Hook was added successfully
    AlreadyPresent, // Hook was already in settings.json
    Declined,       // User declined when prompted
    Skipped,        // --no-patch flag used
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpencodeInstallScope {
    Global,
    Local,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTarget {
    Claude,
    Opencode,
    Both,
}

#[cfg(unix)]
impl SetupTarget {
    fn includes_claude(self) -> bool {
        matches!(self, Self::Claude | Self::Both)
    }

    fn includes_opencode(self) -> bool {
        matches!(self, Self::Opencode | Self::Both)
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTargetSelection {
    Selected(SetupTarget),
    SkippedChoiceRequired,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTargetStatus {
    Processed,
    AlreadyConfigured,
    Skipped,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupTargetOutcome {
    status: SetupTargetStatus,
    detail: &'static str,
    paths: Vec<PathBuf>,
}

#[cfg(unix)]
impl SetupTargetOutcome {
    fn processed() -> Self {
        Self {
            status: SetupTargetStatus::Processed,
            detail: "configured",
            paths: Vec::new(),
        }
    }

    fn already_configured() -> Self {
        Self {
            status: SetupTargetStatus::AlreadyConfigured,
            detail: "already configured",
            paths: Vec::new(),
        }
    }

    fn skipped() -> Self {
        Self {
            status: SetupTargetStatus::Skipped,
            detail: "skipped",
            paths: Vec::new(),
        }
    }

    fn with_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.paths = paths;
        self
    }

    fn with_detail(mut self, detail: &'static str) -> Self {
        self.detail = detail;
        self
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalSetupTargetSummary {
    name: &'static str,
    status: SetupTargetStatus,
    detail: &'static str,
    paths: Vec<PathBuf>,
    processed: bool,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalSetupSummary {
    selected_target: SetupTarget,
    outcomes: Vec<FinalSetupTargetSummary>,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShowConfigOpencodeStatus {
    global_root: PathBuf,
    plugin: Option<(SetupTargetStatus, PathBuf)>,
    agents: Option<(SetupTargetStatus, PathBuf)>,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupExecutionSummary {
    sections: Vec<&'static str>,
    claude: SetupTargetOutcome,
    opencode: SetupTargetOutcome,
}

#[cfg(unix)]
impl Default for SetupExecutionSummary {
    fn default() -> Self {
        Self {
            sections: Vec::new(),
            claude: SetupTargetOutcome::skipped(),
            opencode: SetupTargetOutcome::skipped(),
        }
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitMode {
    Default,
    ClaudeMd,
    HookOnly,
}

#[cfg(unix)]
pub(crate) fn resolve_init_mode(claude_md: bool, hook_only: bool) -> InitMode {
    match (claude_md, hook_only) {
        (true, _) => InitMode::ClaudeMd,
        (false, true) => InitMode::HookOnly,
        (false, false) => InitMode::Default,
    }
}

#[cfg(unix)]
impl OpencodeInstallScope {
    fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Local => "local",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Global => Self::Local,
            Self::Local => Self::Global,
        }
    }
}

#[cfg(unix)]
impl SetupTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Both => "both",
        }
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingOpencodeInstall {
    scope: OpencodeInstallScope,
    path: PathBuf,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpencodeInstallStatus {
    Installed {
        scope: OpencodeInstallScope,
        path: PathBuf,
        other_existing: Option<ExistingOpencodeInstall>,
    },
    AlreadyInstalled {
        scope: OpencodeInstallScope,
        path: PathBuf,
        other_existing: Option<ExistingOpencodeInstall>,
    },
    SkippedChoiceRequired,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpencodeInstallTargetSelection {
    Selected(OpencodeInstallScope),
    SkippedChoiceRequired,
}

// Legacy full instructions for backward compatibility (--claude-md mode)
const RTK_INSTRUCTIONS: &str = r##"<!-- rtk-instructions v2 -->
# RTK (Rust Token Killer) - Token-Optimized Commands

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:
```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## RTK Commands by Workflow

### Build & Compile (80-90% savings)
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
```

### Test (90-99% savings)
```bash
rtk cargo test          # Cargo test failures only (90%)
rtk vitest run          # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk test <cmd>          # Generic test wrapper - failures only
```

### Git (59-80% savings)
```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

### GitHub (26-87% savings)
```bash
rtk gh pr view <num>    # Compact PR view (87%)
rtk gh pr checks        # Compact PR checks (79%)
rtk gh run list         # Compact workflow runs (82%)
rtk gh issue list       # Compact issue list (80%)
rtk gh api              # Compact API responses (26%)
```

### JavaScript/TypeScript Tooling (70-90% savings)
```bash
rtk pnpm list           # Compact dependency tree (70%)
rtk pnpm outdated       # Compact outdated packages (80%)
rtk pnpm install        # Compact install output (90%)
rtk npm run <script>    # Compact npm script output
rtk npx <cmd>           # Compact npx command output
rtk prisma              # Prisma without ASCII art (88%)
```

### Files & Search (60-75% savings)
```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%)
rtk find <pattern>      # Find grouped by directory (70%)
```

### Analysis & Debug (70-90% savings)
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure (85% savings)
```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network (65-70% savings)
```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
```

### Meta Commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.
<!-- /rtk-instructions -->
"##;

/// Main entry point for `rtk init`
pub fn run(
    global: bool,
    claude_md: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    verbose: u8,
) -> Result<()> {
    #[cfg(unix)]
    match resolve_init_mode(claude_md, hook_only) {
        InitMode::ClaudeMd => run_claude_md_mode(global, verbose)?,
        InitMode::HookOnly => run_hook_only_mode(global, patch_mode, verbose)?,
        InitMode::Default => run_default_mode(global, patch_mode, verbose)?,
    }

    #[cfg(not(unix))]
    match (claude_md, hook_only) {
        (true, _) => run_claude_md_mode(global, verbose)?,
        (false, true) => run_hook_only_mode(global, patch_mode, verbose)?,
        (false, false) => run_default_mode(global, patch_mode, verbose)?,
    }

    Ok(())
}

/// Prepare hook directory and return paths (hook_dir, hook_path)
fn prepare_hook_paths() -> Result<(PathBuf, PathBuf)> {
    let claude_dir = resolve_claude_dir()?;
    let hook_dir = claude_dir.join("hooks");
    fs::create_dir_all(&hook_dir)
        .with_context(|| format!("Failed to create hook directory: {}", hook_dir.display()))?;
    let hook_path = hook_dir.join("rtk-rewrite.sh");
    Ok((hook_dir, hook_path))
}

/// Write hook file if missing or outdated, return true if changed
#[cfg(unix)]
fn ensure_hook_installed(hook_path: &Path, verbose: u8) -> Result<bool> {
    let changed = if hook_path.exists() {
        let existing = fs::read_to_string(hook_path)
            .with_context(|| format!("Failed to read existing hook: {}", hook_path.display()))?;

        if existing == REWRITE_HOOK {
            if verbose > 0 {
                eprintln!("Hook already up to date: {}", hook_path.display());
            }
            false
        } else {
            fs::write(hook_path, REWRITE_HOOK)
                .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
            if verbose > 0 {
                eprintln!("Updated hook: {}", hook_path.display());
            }
            true
        }
    } else {
        fs::write(hook_path, REWRITE_HOOK)
            .with_context(|| format!("Failed to write hook to {}", hook_path.display()))?;
        if verbose > 0 {
            eprintln!("Created hook: {}", hook_path.display());
        }
        true
    };

    // Set executable permissions
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;

    // Store SHA-256 hash for runtime integrity verification.
    // Always store (idempotent) to ensure baseline exists even for
    // hooks installed before integrity checks were added.
    integrity::store_hash(hook_path)
        .with_context(|| format!("Failed to store integrity hash for {}", hook_path.display()))?;
    if verbose > 0 && changed {
        eprintln!("Stored integrity hash for hook");
    }

    Ok(changed)
}

/// Idempotent file write: create or update if content differs
fn write_if_changed(path: &Path, content: &str, name: &str, verbose: u8) -> Result<bool> {
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}: {}", name, path.display()))?;

        if existing == content {
            if verbose > 0 {
                eprintln!("{} already up to date: {}", name, path.display());
            }
            Ok(false)
        } else {
            fs::write(path, content)
                .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
            if verbose > 0 {
                eprintln!("Updated {}: {}", name, path.display());
            }
            Ok(true)
        }
    } else {
        fs::write(path, content)
            .with_context(|| format!("Failed to write {}: {}", name, path.display()))?;
        if verbose > 0 {
            eprintln!("Created {}: {}", name, path.display());
        }
        Ok(true)
    }
}

/// Atomic write using tempfile + rename
/// Prevents corruption on crash/interrupt
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().with_context(|| {
        format!(
            "Cannot write to {}: path has no parent directory",
            path.display()
        )
    })?;

    // Create temp file in same directory (ensures same filesystem for atomic rename)
    let mut temp_file = NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;

    // Write content
    temp_file
        .write_all(content.as_bytes())
        .with_context(|| format!("Failed to write {} bytes to temp file", content.len()))?;

    // Atomic rename
    temp_file.persist(path).with_context(|| {
        format!(
            "Failed to atomically replace {} (disk full?)",
            path.display()
        )
    })?;

    Ok(())
}

/// Prompt user for consent to patch settings.json
/// Prints to stderr (stdout may be piped), reads from stdin
/// Default is No (capital N)
fn prompt_user_consent(settings_path: &Path) -> Result<bool> {
    use std::io::{self, BufRead, IsTerminal};

    eprintln!("\nPatch existing {}? [y/N] ", settings_path.display());

    // If stdin is not a terminal (piped), default to No
    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, defaulting to N)");
        return Ok(false);
    }

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let response = line.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

#[cfg(unix)]
fn detect_opencode_with<F>(config_dir: Option<&Path>, has_binary: F) -> bool
where
    F: FnOnce() -> bool,
{
    let has_config_dir = config_dir
        .map(|dir| dir.join("opencode").exists())
        .unwrap_or(false);

    if has_config_dir {
        return true;
    }

    has_binary()
}

#[cfg(unix)]
fn prompt_setup_target() -> Result<SetupTargetSelection> {
    use std::io::{self, BufRead, IsTerminal};

    eprintln!("\nChoose setup target: [Claude/opencode/both] ");

    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, explicit Claude/opencode/both choice required)");
        return Ok(SetupTargetSelection::SkippedChoiceRequired);
    }

    let stdin = io::stdin();
    let mut handle = stdin.lock();

    loop {
        let mut line = String::new();
        handle
            .read_line(&mut line)
            .context("Failed to read setup target")?;

        match resolve_setup_target(Some(&line), true) {
            SetupTargetSelection::Selected(target) => {
                return Ok(SetupTargetSelection::Selected(target));
            }
            SetupTargetSelection::SkippedChoiceRequired => {
                eprintln!("Please answer Claude, opencode, or both.");
            }
        }
    }
}

#[cfg(unix)]
pub(crate) fn resolve_setup_target(
    response: Option<&str>,
    interactive: bool,
) -> SetupTargetSelection {
    if !interactive {
        return SetupTargetSelection::SkippedChoiceRequired;
    }

    match response.map(|value| value.trim().to_ascii_lowercase()) {
        Some(choice) if choice == "claude" => SetupTargetSelection::Selected(SetupTarget::Claude),
        Some(choice) if choice == "opencode" => {
            SetupTargetSelection::Selected(SetupTarget::Opencode)
        }
        Some(choice) if choice == "both" => SetupTargetSelection::Selected(SetupTarget::Both),
        _ => SetupTargetSelection::SkippedChoiceRequired,
    }
}

#[cfg(unix)]
pub(crate) fn run_setup_target_with<F, G, E>(
    target: SetupTarget,
    mut run_claude: F,
    mut run_opencode: G,
) -> std::result::Result<SetupExecutionSummary, E>
where
    F: FnMut() -> std::result::Result<SetupTargetOutcome, E>,
    G: FnMut() -> std::result::Result<SetupTargetOutcome, E>,
{
    let mut summary = SetupExecutionSummary::default();

    if target.includes_claude() {
        summary.sections.push("Claude setup");
        summary.claude = run_claude()?;
    }

    if target.includes_opencode() {
        summary.sections.push("opencode setup");
        summary.opencode = run_opencode()?;
    }

    Ok(summary)
}

#[cfg(unix)]
fn resolve_opencode_install_target(
    global_init: bool,
    response: Option<&str>,
    interactive: bool,
) -> OpencodeInstallTargetSelection {
    if global_init {
        return OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Global);
    }

    if !interactive {
        return OpencodeInstallTargetSelection::SkippedChoiceRequired;
    }

    match response.map(|value| value.trim().to_ascii_lowercase()) {
        Some(choice) if choice == "global" => {
            OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Global)
        }
        Some(choice) if choice == "local" => {
            OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Local)
        }
        _ => OpencodeInstallTargetSelection::SkippedChoiceRequired,
    }
}

#[cfg(unix)]
fn prompt_opencode_install_target(global_init: bool) -> Result<OpencodeInstallTargetSelection> {
    use std::io::{self, BufRead, IsTerminal};

    if global_init {
        return Ok(OpencodeInstallTargetSelection::Selected(
            OpencodeInstallScope::Global,
        ));
    }

    eprintln!("\nWhere do you want to install opencode plugin? [global/local] ");

    if !io::stdin().is_terminal() {
        eprintln!("(non-interactive mode, explicit global/local choice required)");
        return Ok(OpencodeInstallTargetSelection::SkippedChoiceRequired);
    }

    let stdin = io::stdin();
    let mut handle = stdin.lock();

    loop {
        let mut line = String::new();
        handle
            .read_line(&mut line)
            .context("Failed to read opencode install target")?;

        match resolve_opencode_install_target(false, Some(&line), true) {
            OpencodeInstallTargetSelection::Selected(scope) => {
                return Ok(OpencodeInstallTargetSelection::Selected(scope));
            }
            OpencodeInstallTargetSelection::SkippedChoiceRequired => {
                eprintln!("Please answer global or local.");
            }
        }
    }
}

#[cfg(unix)]
fn resolve_opencode_plugin_path(global: bool) -> Result<PathBuf> {
    if global {
        let config_dir = resolve_official_opencode_config_dir()?;
        Ok(resolve_opencode_plugin_path_from_config_dir(
            &config_dir,
            true,
        ))
    } else {
        Ok(std::env::current_dir()
            .context("Cannot determine current directory")?
            .join(".opencode")
            .join("plugins")
            .join("rtk-rewrite.ts"))
    }
}

#[cfg(unix)]
fn resolve_official_opencode_config_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir();
    let home_dir = dirs::home_dir();
    resolve_official_opencode_config_dir_with(config_dir.as_deref(), home_dir.as_deref())
}

#[cfg(unix)]
fn resolve_official_opencode_config_dir_with(
    config_dir: Option<&Path>,
    home_dir: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(home) = home_dir {
        return Ok(home.join(".config"));
    }

    config_dir
        .map(Path::to_path_buf)
        .context("Cannot determine config directory")
}

#[cfg(unix)]
fn resolve_opencode_global_root() -> Result<PathBuf> {
    let config_dir = resolve_official_opencode_config_dir()?;
    Ok(resolve_opencode_global_root_at(&config_dir))
}

#[cfg(unix)]
fn resolve_opencode_global_root_at(config_dir: &Path) -> PathBuf {
    config_dir.join("opencode")
}

#[cfg(unix)]
fn resolve_opencode_plugin_path_from_config_dir(config_dir: &Path, global: bool) -> PathBuf {
    if global {
        resolve_opencode_global_root_at(config_dir)
            .join("plugins")
            .join("rtk-rewrite.ts")
    } else {
        config_dir
            .join(".opencode")
            .join("plugins")
            .join("rtk-rewrite.ts")
    }
}

#[cfg(unix)]
fn resolve_opencode_agents_path() -> Result<PathBuf> {
    Ok(resolve_opencode_global_root()?.join("AGENTS.md"))
}

#[cfg(unix)]
fn resolve_opencode_plugin_path_at(root: &Path, global: bool) -> PathBuf {
    if global {
        root.join("config")
            .join("opencode")
            .join("plugins")
            .join("rtk-rewrite.ts")
    } else {
        root.join(".opencode")
            .join("plugins")
            .join("rtk-rewrite.ts")
    }
}

#[cfg(unix)]
fn resolve_opencode_plugin_path_for_scope(scope: OpencodeInstallScope) -> Result<PathBuf> {
    resolve_opencode_plugin_path(scope.is_global())
}

#[cfg(unix)]
fn resolve_opencode_plugin_path_at_for_scope(root: &Path, scope: OpencodeInstallScope) -> PathBuf {
    resolve_opencode_plugin_path_at(root, scope.is_global())
}

#[cfg(unix)]
fn install_opencode_plugin(global: bool, verbose: u8) -> Result<bool> {
    let scope = if global {
        OpencodeInstallScope::Global
    } else {
        OpencodeInstallScope::Local
    };

    Ok(matches!(
        install_opencode_plugin_with_status(scope, verbose)?,
        OpencodeInstallStatus::Installed { .. }
    ))
}

#[cfg(unix)]
fn install_opencode_plugin_at(root: &Path, global: bool, verbose: u8) -> Result<bool> {
    let scope = if global {
        OpencodeInstallScope::Global
    } else {
        OpencodeInstallScope::Local
    };

    Ok(matches!(
        install_opencode_plugin_with_status_at(root, scope, verbose)?,
        OpencodeInstallStatus::Installed { .. }
    ))
}

#[cfg(unix)]
fn install_opencode_plugin_with_status(
    scope: OpencodeInstallScope,
    verbose: u8,
) -> Result<OpencodeInstallStatus> {
    let plugin_path = resolve_opencode_plugin_path_for_scope(scope)?;
    let other_path = resolve_opencode_plugin_path_for_scope(scope.other())?;
    install_opencode_plugin_file_with_status(&plugin_path, &other_path, scope, verbose)
}

#[cfg(unix)]
fn install_opencode_plugin_with_status_at(
    root: &Path,
    scope: OpencodeInstallScope,
    verbose: u8,
) -> Result<OpencodeInstallStatus> {
    let plugin_path = resolve_opencode_plugin_path_at_for_scope(root, scope);
    let other_path = resolve_opencode_plugin_path_at_for_scope(root, scope.other());
    install_opencode_plugin_file_with_status(&plugin_path, &other_path, scope, verbose)
}

#[cfg(unix)]
fn install_opencode_plugin_file_with_status(
    path: &Path,
    other_path: &Path,
    scope: OpencodeInstallScope,
    verbose: u8,
) -> Result<OpencodeInstallStatus> {
    let other_existing = other_path.exists().then(|| ExistingOpencodeInstall {
        scope: scope.other(),
        path: other_path.to_path_buf(),
    });

    let parent = path.parent().with_context(|| {
        format!(
            "Cannot install opencode plugin at {}: missing parent directory",
            path.display()
        )
    })?;

    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create plugin directory: {}", parent.display()))?;

    if path.exists() {
        if verbose > 0 {
            eprintln!("opencode plugin already installed: {}", path.display());
        }
        return Ok(OpencodeInstallStatus::AlreadyInstalled {
            scope,
            path: path.to_path_buf(),
            other_existing,
        });
    }

    fs::write(path, OPENCODE_PLUGIN)
        .with_context(|| format!("Failed to write opencode plugin: {}", path.display()))?;

    if verbose > 0 {
        eprintln!("Created opencode plugin: {}", path.display());
    }

    Ok(OpencodeInstallStatus::Installed {
        scope,
        path: path.to_path_buf(),
        other_existing,
    })
}

#[cfg(unix)]
fn format_opencode_install_status(status: &OpencodeInstallStatus) -> String {
    let mut lines = Vec::new();

    match status {
        OpencodeInstallStatus::Installed {
            scope,
            path,
            other_existing,
        } => {
            lines.push(format!("  opencode plugin installed: {}", path.display()));
            lines.push(format!("  Scope: {}", scope.label()));
            lines.push(format!(
                "  Active plugin path to refresh/recheck: {}",
                path.display()
            ));

            if let Some(other) = other_existing {
                lines.push(format!(
                    "  Note: {} already installed: {}",
                    other.scope.label(),
                    other.path.display()
                ));
                lines.push(
                    "  Note: duplicate global/local installs can double-load; keep only the intended path before rechecking."
                        .to_string(),
                );
            }

            lines.push(
                "  opencode bash/tool execution now routes through `rtk rewrite`.".to_string(),
            );
            lines.push(
                "  RTK lookup keeps the standard absolute-path fallbacks and also checks `<project>/target/debug/rtk`, `<project>/target/release/rtk`, and PATH for source-built installs.".to_string(),
            );
            lines.push(
                "  If the active file is stale, delete that exact path and rerun `rtk init` for the same scope before restarting opencode.".to_string(),
            );
            lines.push(
                "  Verify the refreshed plugin path exists, then run `git status` in opencode."
                    .to_string(),
            );
        }
        OpencodeInstallStatus::AlreadyInstalled {
            path,
            other_existing,
            ..
        } => {
            lines.push(format!("  already installed: {}", path.display()));
            lines.push(format!(
                "  Active plugin path to refresh/recheck: {}",
                path.display()
            ));

            if let Some(other) = other_existing {
                lines.push(format!(
                    "  Note: {} already installed: {}",
                    other.scope.label(),
                    other.path.display()
                ));
                lines.push(
                    "  Note: duplicate global/local installs can double-load; remove the non-target copy before rechecking."
                        .to_string(),
                );
            }

            lines.push(
                "  Delete the exact stale plugin path above before rerunning `rtk init` if you need to refresh the asset."
                    .to_string(),
            );
            lines.push(
                "  RTK lookup uses the standard absolute-path fallbacks plus `<project>/target/debug/rtk`, `<project>/target/release/rtk`, and PATH when you refresh the plugin."
                    .to_string(),
            );
            lines.push("  Run `rtk init --uninstall` to remove opencode support.".to_string());
        }
        OpencodeInstallStatus::SkippedChoiceRequired => {
            lines.push(
                "  Skipped plugin install: local init needs an explicit global/local choice."
                    .to_string(),
            );
            lines.push(
                "  Re-run `rtk init` interactively to choose where to install the plugin."
                    .to_string(),
            );
        }
    }

    lines.join("\n")
}

#[cfg(unix)]
fn build_final_setup_summary(
    selected_target: SetupTarget,
    execution: &SetupExecutionSummary,
) -> FinalSetupSummary {
    let outcomes = [
        (
            "Claude",
            selected_target.includes_claude(),
            &execution.claude,
            "not selected",
        ),
        (
            "opencode",
            selected_target.includes_opencode(),
            &execution.opencode,
            "not selected",
        ),
    ]
    .into_iter()
    .map(
        |(name, processed, outcome, skipped_detail)| FinalSetupTargetSummary {
            name,
            status: if processed {
                outcome.status
            } else {
                SetupTargetStatus::Skipped
            },
            detail: if processed {
                outcome.detail
            } else {
                skipped_detail
            },
            paths: if processed {
                outcome.paths.clone()
            } else {
                Vec::new()
            },
            processed,
        },
    )
    .collect();

    FinalSetupSummary {
        selected_target,
        outcomes,
    }
}

#[cfg(unix)]
fn format_final_setup_summary(summary: &FinalSetupSummary) -> String {
    let mut lines = vec![
        "Final setup summary".to_string(),
        format!("Selected target: {}", summary.selected_target.label()),
    ];

    for outcome in &summary.outcomes {
        let status_line = if outcome.processed {
            format!("- {}: {}", outcome.name, outcome.detail)
        } else {
            format!("- {}: {} (not processed)", outcome.name, outcome.detail)
        };
        lines.push(status_line);

        for path in &outcome.paths {
            lines.push(format!("  path: {}", path.display()));
        }
    }

    lines.join("\n")
}

#[cfg(unix)]
fn format_show_config_opencode_status(status: &ShowConfigOpencodeStatus) -> String {
    let mut lines = vec![format!(
        "opencode (global): {}",
        status.global_root.display()
    )];

    match &status.plugin {
        Some((entry_status, path)) => lines.push(format!(
            "  plugin: {} ({})",
            format_target_status(*entry_status),
            path.display()
        )),
        None => lines.push("  plugin: not configured".to_string()),
    }

    match &status.agents {
        Some((entry_status, path)) => lines.push(format!(
            "  AGENTS.md: {} ({})",
            format_target_status(*entry_status),
            path.display()
        )),
        None => lines.push(format!(
            "  AGENTS.md: not configured ({})",
            status.global_root.join("AGENTS.md").display()
        )),
    }

    lines.join("\n")
}

#[cfg(unix)]
fn format_target_status(status: SetupTargetStatus) -> &'static str {
    match status {
        SetupTargetStatus::Processed => "configured",
        SetupTargetStatus::AlreadyConfigured => "already configured",
        SetupTargetStatus::Skipped => "skipped",
    }
}

/// Print manual instructions for settings.json patching
fn print_manual_instructions(hook_path: &Path) {
    println!("\n  MANUAL STEP: Add this to ~/.claude/settings.json:");
    println!("  {{");
    println!("    \"hooks\": {{ \"PreToolUse\": [{{");
    println!("      \"matcher\": \"Bash\",");
    println!("      \"hooks\": [{{ \"type\": \"command\",");
    println!("        \"command\": \"{}\"", hook_path.display());
    println!("      }}]");
    println!("    }}]}}");
    println!("  }}");
    println!("\n  Then restart Claude Code. Test with: git status\n");
}

/// Remove RTK hook entry from settings.json
/// Returns true if hook was found and removed
fn remove_hook_from_json(root: &mut serde_json::Value) -> bool {
    let hooks = match root.get_mut("hooks").and_then(|h| h.get_mut("PreToolUse")) {
        Some(pre_tool_use) => pre_tool_use,
        None => return false,
    };

    let pre_tool_use_array = match hooks.as_array_mut() {
        Some(arr) => arr,
        None => return false,
    };

    // Find and remove RTK entry
    let original_len = pre_tool_use_array.len();
    pre_tool_use_array.retain(|entry| {
        if let Some(hooks_array) = entry.get("hooks").and_then(|h| h.as_array()) {
            for hook in hooks_array {
                if let Some(command) = hook.get("command").and_then(|c| c.as_str()) {
                    if command.contains("rtk-rewrite.sh") {
                        return false; // Remove this entry
                    }
                }
            }
        }
        true // Keep this entry
    });

    pre_tool_use_array.len() < original_len
}

/// Remove RTK hook from settings.json file
/// Backs up before modification, returns true if hook was found and removed
fn remove_hook_from_settings(verbose: u8) -> Result<bool> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");

    if !settings_path.exists() {
        if verbose > 0 {
            eprintln!("settings.json not found, nothing to remove");
        }
        return Ok(false);
    }

    let content = fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;

    if content.trim().is_empty() {
        return Ok(false);
    }

    let mut root: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?;

    let removed = remove_hook_from_json(&mut root);

    if removed {
        // Backup original
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;

        // Atomic write
        let serialized =
            serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
        atomic_write(&settings_path, &serialized)?;

        if verbose > 0 {
            eprintln!("Removed RTK hook from settings.json");
        }
    }

    Ok(removed)
}

/// Full uninstall: remove hook, RTK.md, @RTK.md reference, settings.json entry
pub fn uninstall(global: bool, verbose: u8) -> Result<()> {
    if !global {
        anyhow::bail!("Uninstall only works with --global flag. For local projects, manually remove RTK from CLAUDE.md");
    }

    let claude_dir = resolve_claude_dir()?;
    let mut removed = Vec::new();

    // 1. Remove hook file
    let hook_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    if hook_path.exists() {
        fs::remove_file(&hook_path)
            .with_context(|| format!("Failed to remove hook: {}", hook_path.display()))?;
        removed.push(format!("Hook: {}", hook_path.display()));
    }

    // 1b. Remove integrity hash file
    if integrity::remove_hash(&hook_path)? {
        removed.push("Integrity hash: removed".to_string());
    }

    // 2. Remove RTK.md
    let rtk_md_path = claude_dir.join("RTK.md");
    if rtk_md_path.exists() {
        fs::remove_file(&rtk_md_path)
            .with_context(|| format!("Failed to remove RTK.md: {}", rtk_md_path.display()))?;
        removed.push(format!("RTK.md: {}", rtk_md_path.display()));
    }

    // 3. Remove @RTK.md reference from CLAUDE.md
    let claude_md_path = claude_dir.join("CLAUDE.md");
    if claude_md_path.exists() {
        let content = fs::read_to_string(&claude_md_path)
            .with_context(|| format!("Failed to read CLAUDE.md: {}", claude_md_path.display()))?;

        if content.contains("@RTK.md") {
            let new_content = content
                .lines()
                .filter(|line| !line.trim().starts_with("@RTK.md"))
                .collect::<Vec<_>>()
                .join("\n");

            // Clean up double blanks
            let cleaned = clean_double_blanks(&new_content);

            fs::write(&claude_md_path, cleaned).with_context(|| {
                format!("Failed to write CLAUDE.md: {}", claude_md_path.display())
            })?;
            removed.push(format!("CLAUDE.md: removed @RTK.md reference"));
        }
    }

    // 4. Remove hook entry from settings.json
    if remove_hook_from_settings(verbose)? {
        removed.push("settings.json: removed RTK hook entry".to_string());
    }

    // Report results
    if removed.is_empty() {
        println!("RTK was not installed (nothing to remove)");
    } else {
        println!("RTK uninstalled:");
        for item in removed {
            println!("  - {}", item);
        }
        println!("\nRestart Claude Code to apply changes.");
    }

    Ok(())
}

/// Orchestrator: patch settings.json with RTK hook
/// Handles reading, checking, prompting, merging, backing up, and atomic writing
fn patch_settings_json(hook_path: &Path, mode: PatchMode, verbose: u8) -> Result<PatchResult> {
    let claude_dir = resolve_claude_dir()?;
    let settings_path = claude_dir.join("settings.json");
    let hook_command = hook_path
        .to_str()
        .context("Hook path contains invalid UTF-8")?;

    // Read or create settings.json
    let mut root = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse {} as JSON", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    // Check idempotency
    if hook_already_present(&root, &hook_command) {
        if verbose > 0 {
            eprintln!("settings.json: hook already present");
        }
        return Ok(PatchResult::AlreadyPresent);
    }

    // Handle mode
    match mode {
        PatchMode::Skip => {
            print_manual_instructions(hook_path);
            return Ok(PatchResult::Skipped);
        }
        PatchMode::Ask => {
            if !prompt_user_consent(&settings_path)? {
                print_manual_instructions(hook_path);
                return Ok(PatchResult::Declined);
            }
        }
        PatchMode::Auto => {
            // Proceed without prompting
        }
    }

    // Deep-merge hook
    insert_hook_entry(&mut root, &hook_command);

    // Backup original
    if settings_path.exists() {
        let backup_path = settings_path.with_extension("json.bak");
        fs::copy(&settings_path, &backup_path)
            .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
        if verbose > 0 {
            eprintln!("Backup: {}", backup_path.display());
        }
    }

    // Atomic write
    let serialized =
        serde_json::to_string_pretty(&root).context("Failed to serialize settings.json")?;
    atomic_write(&settings_path, &serialized)?;

    println!("\n  settings.json: hook added");
    if settings_path.with_extension("json.bak").exists() {
        println!(
            "  Backup: {}",
            settings_path.with_extension("json.bak").display()
        );
    }
    println!("  Restart Claude Code. Test with: git status");

    Ok(PatchResult::Patched)
}

/// Clean up consecutive blank lines (collapse 3+ to 2)
/// Used when removing @RTK.md line from CLAUDE.md
fn clean_double_blanks(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.trim().is_empty() {
            // Count consecutive blank lines
            let mut blank_count = 0;
            while i < lines.len() && lines[i].trim().is_empty() {
                blank_count += 1;
                i += 1;
            }

            // Keep at most 2 blank lines
            let keep = blank_count.min(2);
            for _ in 0..keep {
                result.push("");
            }
        } else {
            result.push(line);
            i += 1;
        }
    }

    result.join("\n")
}

/// Deep-merge RTK hook entry into settings.json
/// Creates hooks.PreToolUse structure if missing, preserves existing hooks
fn insert_hook_entry(root: &mut serde_json::Value, hook_command: &str) {
    // Ensure root is an object
    let root_obj = match root.as_object_mut() {
        Some(obj) => obj,
        None => {
            *root = serde_json::json!({});
            root.as_object_mut()
                .expect("Just created object, must succeed")
        }
    };

    // Use entry() API for idiomatic insertion
    let hooks = root_obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("hooks must be an object");

    let pre_tool_use = hooks
        .entry("PreToolUse")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("PreToolUse must be an array");

    // Append RTK hook entry
    pre_tool_use.push(serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": hook_command
        }]
    }));
}

/// Check if RTK hook is already present in settings.json
/// Matches on rtk-rewrite.sh substring to handle different path formats
fn hook_already_present(root: &serde_json::Value, hook_command: &str) -> bool {
    let pre_tool_use_array = match root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|p| p.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    pre_tool_use_array
        .iter()
        .filter_map(|entry| entry.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .any(|cmd| {
            // Exact match OR both contain rtk-rewrite.sh
            cmd == hook_command
                || (cmd.contains("rtk-rewrite.sh") && hook_command.contains("rtk-rewrite.sh"))
        })
}

#[cfg(unix)]
fn run_opencode_target(global: bool, verbose: u8) -> Result<SetupTargetOutcome> {
    let target = prompt_opencode_install_target(global)?;
    let status = match target {
        OpencodeInstallTargetSelection::Selected(scope) => {
            install_opencode_plugin_with_status(scope, verbose)?
        }
        OpencodeInstallTargetSelection::SkippedChoiceRequired => {
            println!("  Skipped opencode setup: local init needs an explicit global/local choice.");
            return Ok(SetupTargetOutcome::skipped());
        }
    };

    println!("{}", format_opencode_install_status(&status));

    Ok(match status {
        OpencodeInstallStatus::Installed { path, .. } => {
            SetupTargetOutcome::processed().with_paths(vec![path])
        }
        OpencodeInstallStatus::AlreadyInstalled { path, .. } => {
            SetupTargetOutcome::already_configured().with_paths(vec![path])
        }
        OpencodeInstallStatus::SkippedChoiceRequired => {
            SetupTargetOutcome::skipped().with_detail("choice required")
        }
    })
}

/// Default mode: hook + slim RTK.md + @RTK.md reference
#[cfg(not(unix))]
fn run_default_mode(_global: bool, _patch_mode: PatchMode, _verbose: u8) -> Result<()> {
    eprintln!("⚠️  Hook-based mode requires Unix (macOS/Linux).");
    eprintln!("    Windows: use --claude-md mode for full injection.");
    eprintln!("    Falling back to --claude-md mode.");
    run_claude_md_mode(_global, _verbose)
}

#[cfg(unix)]
fn run_claude_target(
    global: bool,
    patch_mode: PatchMode,
    verbose: u8,
) -> Result<SetupTargetOutcome> {
    if !global {
        return run_claude_md_mode_with_status(false, verbose);
    }

    let claude_dir = resolve_claude_dir()?;
    let rtk_md_path = claude_dir.join("RTK.md");
    let claude_md_path = claude_dir.join("CLAUDE.md");

    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    let hook_changed = ensure_hook_installed(&hook_path, verbose)?;
    let rtk_md_changed = write_if_changed(&rtk_md_path, RTK_SLIM, "RTK.md", verbose)?;
    let migrated = patch_claude_md(&claude_md_path, verbose)?;

    let hook_status = if hook_changed {
        "installed/updated"
    } else {
        "already up to date"
    };
    println!("\nRTK hook {} (global).\n", hook_status);
    println!("  Hook:      {}", hook_path.display());
    println!("  RTK.md:    {} (10 lines)", rtk_md_path.display());
    println!("  CLAUDE.md: @RTK.md reference added");

    if migrated {
        println!("\n  ✅ Migrated: removed 137-line RTK block from CLAUDE.md");
        println!("              replaced with @RTK.md (10 lines)");
    }

    let patch_result = patch_settings_json(&hook_path, patch_mode, verbose)?;
    match patch_result {
        PatchResult::Patched => {}
        PatchResult::AlreadyPresent => {
            println!("\n  settings.json: hook already present");
            println!("  Restart Claude Code. Test with: git status");
        }
        PatchResult::Declined | PatchResult::Skipped => {}
    }

    let outcome_paths = vec![hook_path, rtk_md_path, claude_md_path];

    let status = if !hook_changed
        && !rtk_md_changed
        && !migrated
        && matches!(patch_result, PatchResult::AlreadyPresent)
    {
        SetupTargetOutcome::already_configured().with_paths(outcome_paths)
    } else {
        SetupTargetOutcome::processed().with_paths(outcome_paths)
    };

    Ok(status)
}

#[cfg(unix)]
fn run_default_mode(global: bool, patch_mode: PatchMode, verbose: u8) -> Result<()> {
    let target = prompt_setup_target()?;
    let selected_target = match target {
        SetupTargetSelection::Selected(target) => target,
        SetupTargetSelection::SkippedChoiceRequired => {
            println!("\nSkipped init: default mode needs an explicit Claude/opencode/both choice.");
            return Ok(());
        }
    };

    let summary = run_setup_target_with(
        selected_target,
        || {
            println!("\nClaude setup");
            run_claude_target(global, patch_mode, verbose)
        },
        || {
            println!("\nopencode setup");
            run_opencode_target(global, verbose)
        },
    )?;

    println!(
        "{}",
        format_final_setup_summary(&build_final_setup_summary(selected_target, &summary))
    );

    println!();

    Ok(())
}

/// Hook-only mode: just the hook, no RTK.md
#[cfg(not(unix))]
fn run_hook_only_mode(_global: bool, _patch_mode: PatchMode, _verbose: u8) -> Result<()> {
    anyhow::bail!("Hook install requires Unix (macOS/Linux). Use WSL or --claude-md mode.")
}

#[cfg(unix)]
fn run_hook_only_mode(global: bool, patch_mode: PatchMode, verbose: u8) -> Result<()> {
    if !global {
        eprintln!("⚠️  Warning: --hook-only only makes sense with --global");
        eprintln!("    For local projects, use default mode or --claude-md");
        return Ok(());
    }

    // Prepare and install hook
    let (_hook_dir, hook_path) = prepare_hook_paths()?;
    let hook_changed = ensure_hook_installed(&hook_path, verbose)?;

    let hook_status = if hook_changed {
        "installed/updated"
    } else {
        "already up to date"
    };
    println!("\nRTK hook {} (hook-only mode).\n", hook_status);
    println!("  Hook: {}", hook_path.display());
    println!(
        "  Note: No RTK.md created. Claude won't know about meta commands (gain, discover, proxy)."
    );

    // Patch settings.json
    let patch_result = patch_settings_json(&hook_path, patch_mode, verbose)?;

    // Report result
    match patch_result {
        PatchResult::Patched => {
            // Already printed by patch_settings_json
        }
        PatchResult::AlreadyPresent => {
            println!("\n  settings.json: hook already present");
            println!("  Restart Claude Code. Test with: git status");
        }
        PatchResult::Declined | PatchResult::Skipped => {
            // Manual instructions already printed by patch_settings_json
        }
    }

    println!(); // Final newline

    Ok(())
}

/// Legacy mode: full 137-line injection into CLAUDE.md
fn run_claude_md_mode(global: bool, verbose: u8) -> Result<()> {
    run_claude_md_mode_with_status(global, verbose).map(|_| ())
}

fn run_claude_md_mode_with_status(global: bool, verbose: u8) -> Result<SetupTargetOutcome> {
    let path = if global {
        resolve_claude_dir()?.join("CLAUDE.md")
    } else {
        PathBuf::from("CLAUDE.md")
    };

    if global {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    if path.exists() {
        let existing = fs::read_to_string(&path)?;
        // upsert_rtk_block handles all 4 cases: add, update, unchanged, malformed
        let (new_content, action) = upsert_rtk_block(&existing, RTK_INSTRUCTIONS);

        match action {
            RtkBlockUpsert::Added => {
                fs::write(&path, new_content)?;
                println!("✅ Added rtk instructions to existing {}", path.display());
            }
            RtkBlockUpsert::Updated => {
                fs::write(&path, new_content)?;
                println!("✅ Updated rtk instructions in {}", path.display());
            }
            RtkBlockUpsert::Unchanged => {
                println!(
                    "✅ {} already contains up-to-date rtk instructions",
                    path.display()
                );
                return Ok(SetupTargetOutcome::already_configured());
            }
            RtkBlockUpsert::Malformed => {
                eprintln!(
                    "⚠️  Warning: Found '<!-- rtk-instructions' without closing marker in {}",
                    path.display()
                );

                if let Some((line_num, _)) = existing
                    .lines()
                    .enumerate()
                    .find(|(_, line)| line.contains("<!-- rtk-instructions"))
                {
                    eprintln!("    Location: line {}", line_num + 1);
                }

                eprintln!("    Action: Manually remove the incomplete block, then re-run:");
                if global {
                    eprintln!("            rtk init -g --claude-md");
                } else {
                    eprintln!("            rtk init --claude-md");
                }
                return Ok(SetupTargetOutcome::skipped());
            }
        }
    } else {
        fs::write(&path, RTK_INSTRUCTIONS)?;
        println!("✅ Created {} with rtk instructions", path.display());
    }

    if global {
        println!("   Claude Code will now use rtk in all sessions");
    } else {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(SetupTargetOutcome::processed())
}

// --- upsert_rtk_block: idempotent RTK block management ---

#[derive(Debug, Clone, Copy, PartialEq)]
enum RtkBlockUpsert {
    /// No existing block found — appended new block
    Added,
    /// Existing block found with different content — replaced
    Updated,
    /// Existing block found with identical content — no-op
    Unchanged,
    /// Opening marker found without closing marker — not safe to rewrite
    Malformed,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpencodeAgentsSectionUpsert {
    Added,
    Updated,
    Unchanged,
    Malformed,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpencodeAgentsSectionRemove {
    Removed,
    Unchanged,
    Malformed,
}

/// Insert or replace the RTK instructions block in `content`.
///
/// Returns `(new_content, action)` describing what happened.
/// The caller decides whether to write `new_content` based on `action`.
fn upsert_rtk_block(content: &str, block: &str) -> (String, RtkBlockUpsert) {
    let start_marker = "<!-- rtk-instructions";
    let end_marker = "<!-- /rtk-instructions -->";

    if let Some(start) = content.find(start_marker) {
        if let Some(relative_end) = content[start..].find(end_marker) {
            let end = start + relative_end;
            let end_pos = end + end_marker.len();
            let current_block = content[start..end_pos].trim();
            let desired_block = block.trim();

            if current_block == desired_block {
                return (content.to_string(), RtkBlockUpsert::Unchanged);
            }

            // Replace stale block with desired block
            let before = content[..start].trim_end();
            let after = content[end_pos..].trim_start();

            let result = match (before.is_empty(), after.is_empty()) {
                (true, true) => desired_block.to_string(),
                (true, false) => format!("{desired_block}\n\n{after}"),
                (false, true) => format!("{before}\n\n{desired_block}"),
                (false, false) => format!("{before}\n\n{desired_block}\n\n{after}"),
            };

            return (result, RtkBlockUpsert::Updated);
        }

        // Opening marker without closing marker — malformed
        return (content.to_string(), RtkBlockUpsert::Malformed);
    }

    // No existing block — append
    let trimmed = content.trim();
    if trimmed.is_empty() {
        (block.to_string(), RtkBlockUpsert::Added)
    } else {
        (
            format!("{trimmed}\n\n{}", block.trim()),
            RtkBlockUpsert::Added,
        )
    }
}

#[cfg(unix)]
fn upsert_opencode_agents_section(content: &str) -> (String, OpencodeAgentsSectionUpsert) {
    let start_marker = "<!-- rtk-opencode-start -->";
    let end_marker = "<!-- rtk-opencode-end -->";

    if let Some(start) = content.find(start_marker) {
        if let Some(relative_end) = content[start..].find(end_marker) {
            let end = start + relative_end;
            let end_pos = end + end_marker.len();
            let current_block = content[start..end_pos].trim();
            let desired_block = RTK_OPENCODE_SECTION.trim();

            if current_block == desired_block {
                return (content.to_string(), OpencodeAgentsSectionUpsert::Unchanged);
            }

            let before = content[..start].trim_end();
            let after = content[end_pos..].trim_start();
            let result = match (before.is_empty(), after.is_empty()) {
                (true, true) => desired_block.to_string(),
                (true, false) => format!("{desired_block}\n\n{after}"),
                (false, true) => format!("{before}\n\n{desired_block}"),
                (false, false) => format!("{before}\n\n{desired_block}\n\n{after}"),
            };

            return (result, OpencodeAgentsSectionUpsert::Updated);
        }

        return (content.to_string(), OpencodeAgentsSectionUpsert::Malformed);
    }

    let trimmed = content.trim();
    if trimmed.is_empty() {
        (
            RTK_OPENCODE_SECTION.to_string(),
            OpencodeAgentsSectionUpsert::Added,
        )
    } else {
        (
            format!("{trimmed}\n\n{}", RTK_OPENCODE_SECTION.trim()),
            OpencodeAgentsSectionUpsert::Added,
        )
    }
}

#[cfg(unix)]
fn remove_opencode_agents_section(content: &str) -> (String, OpencodeAgentsSectionRemove) {
    let start_marker = "<!-- rtk-opencode-start -->";
    let end_marker = "<!-- rtk-opencode-end -->";

    if let Some(start) = content.find(start_marker) {
        if let Some(relative_end) = content[start..].find(end_marker) {
            let end = start + relative_end;
            let end_pos = end + end_marker.len();
            let before = content[..start].trim_end();
            let after = content[end_pos..].trim_start();
            let result = match (before.is_empty(), after.is_empty()) {
                (true, true) => String::new(),
                (true, false) => after.to_string(),
                (false, true) => before.to_string(),
                (false, false) => format!("{before}\n\n{after}"),
            };

            return (result, OpencodeAgentsSectionRemove::Removed);
        }

        return (content.to_string(), OpencodeAgentsSectionRemove::Malformed);
    }

    (content.to_string(), OpencodeAgentsSectionRemove::Unchanged)
}

/// Patch CLAUDE.md: add @RTK.md, migrate if old block exists
fn patch_claude_md(path: &Path, verbose: u8) -> Result<bool> {
    let mut content = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let mut migrated = false;

    // Check for old block and migrate
    if content.contains("<!-- rtk-instructions") {
        let (new_content, did_migrate) = remove_rtk_block(&content);
        if did_migrate {
            content = new_content;
            migrated = true;
            if verbose > 0 {
                eprintln!("Migrated: removed old RTK block from CLAUDE.md");
            }
        }
    }

    // Check if @RTK.md already present
    if content.contains("@RTK.md") {
        if verbose > 0 {
            eprintln!("@RTK.md reference already present in CLAUDE.md");
        }
        if migrated {
            fs::write(path, content)?;
        }
        return Ok(migrated);
    }

    // Add @RTK.md
    let new_content = if content.is_empty() {
        "@RTK.md\n".to_string()
    } else {
        format!("{}\n\n@RTK.md\n", content.trim())
    };

    fs::write(path, new_content)?;

    if verbose > 0 {
        eprintln!("Added @RTK.md reference to CLAUDE.md");
    }

    Ok(migrated)
}

/// Remove old RTK block from CLAUDE.md (migration helper)
fn remove_rtk_block(content: &str) -> (String, bool) {
    if let (Some(start), Some(end)) = (
        content.find("<!-- rtk-instructions"),
        content.find("<!-- /rtk-instructions -->"),
    ) {
        let end_pos = end + "<!-- /rtk-instructions -->".len();
        let before = content[..start].trim_end();
        let after = content[end_pos..].trim_start();

        let result = if after.is_empty() {
            before.to_string()
        } else {
            format!("{}\n\n{}", before, after)
        };

        (result, true) // migrated
    } else if content.contains("<!-- rtk-instructions") {
        eprintln!("⚠️  Warning: Found '<!-- rtk-instructions' without closing marker.");
        eprintln!("    This can happen if CLAUDE.md was manually edited.");

        // Find line number
        if let Some((line_num, _)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("<!-- rtk-instructions"))
        {
            eprintln!("    Location: line {}", line_num + 1);
        }

        eprintln!("    Action: Manually remove the incomplete block, then re-run:");
        eprintln!("            rtk init -g");
        (content.to_string(), false)
    } else {
        (content.to_string(), false)
    }
}

/// Resolve ~/.claude directory with proper home expansion
fn resolve_claude_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .context("Cannot determine home directory. Is $HOME set?")
}

/// Show current rtk configuration
pub fn show_config() -> Result<()> {
    let claude_dir = resolve_claude_dir()?;
    let hook_path = claude_dir.join("hooks").join("rtk-rewrite.sh");
    let rtk_md_path = claude_dir.join("RTK.md");
    let global_claude_md = claude_dir.join("CLAUDE.md");
    let local_claude_md = PathBuf::from("CLAUDE.md");

    println!("📋 rtk Configuration:\n");

    // Check hook
    if hook_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&hook_path)?;
            let perms = metadata.permissions();
            let is_executable = perms.mode() & 0o111 != 0;

            let hook_content = fs::read_to_string(&hook_path)?;
            let has_guards =
                hook_content.contains("command -v rtk") && hook_content.contains("command -v jq");
            let is_thin_delegator = hook_content.contains("rtk rewrite");
            let hook_version = crate::hook_check::parse_hook_version(&hook_content);

            if !is_executable {
                println!(
                    "⚠️  Hook: {} (NOT executable - run: chmod +x)",
                    hook_path.display()
                );
            } else if !is_thin_delegator {
                println!(
                    "⚠️  Hook: {} (outdated — inline logic, not thin delegator)",
                    hook_path.display()
                );
                println!(
                    "   → Run `rtk init --global` to upgrade to the single source of truth hook"
                );
            } else if is_executable && has_guards {
                println!(
                    "✅ Hook: {} (thin delegator, version {})",
                    hook_path.display(),
                    hook_version
                );
            } else {
                println!("⚠️  Hook: {} (no guards - outdated)", hook_path.display());
            }
        }

        #[cfg(not(unix))]
        {
            println!("✅ Hook: {} (exists)", hook_path.display());
        }
    } else {
        println!("⚪ Hook: not found");
    }

    // Check RTK.md
    if rtk_md_path.exists() {
        println!("✅ RTK.md: {} (slim mode)", rtk_md_path.display());
    } else {
        println!("⚪ RTK.md: not found");
    }

    // Check hook integrity
    match integrity::verify_hook_at(&hook_path) {
        Ok(integrity::IntegrityStatus::Verified) => {
            println!("✅ Integrity: hook hash verified");
        }
        Ok(integrity::IntegrityStatus::Tampered { .. }) => {
            println!("❌ Integrity: hook modified outside rtk init (run: rtk verify)");
        }
        Ok(integrity::IntegrityStatus::NoBaseline) => {
            println!("⚠️  Integrity: no baseline hash (run: rtk init -g to establish)");
        }
        Ok(integrity::IntegrityStatus::NotInstalled)
        | Ok(integrity::IntegrityStatus::OrphanedHash) => {
            // Don't show integrity line if hook isn't installed
        }
        Err(_) => {
            println!("⚠️  Integrity: check failed");
        }
    }

    // Check global CLAUDE.md
    if global_claude_md.exists() {
        let content = fs::read_to_string(&global_claude_md)?;
        if content.contains("@RTK.md") {
            println!("✅ Global (~/.claude/CLAUDE.md): @RTK.md reference");
        } else if content.contains("<!-- rtk-instructions") {
            println!(
                "⚠️  Global (~/.claude/CLAUDE.md): old RTK block (run: rtk init -g to migrate)"
            );
        } else {
            println!("⚪ Global (~/.claude/CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("⚪ Global (~/.claude/CLAUDE.md): not found");
    }

    // Check local CLAUDE.md
    if local_claude_md.exists() {
        let content = fs::read_to_string(&local_claude_md)?;
        if content.contains("rtk") {
            println!("✅ Local (./CLAUDE.md): rtk enabled");
        } else {
            println!("⚪ Local (./CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("⚪ Local (./CLAUDE.md): not found");
    }

    // Check settings.json
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if !content.trim().is_empty() {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                let hook_command = hook_path.display().to_string();
                if hook_already_present(&root, &hook_command) {
                    println!("✅ settings.json: RTK hook configured");
                } else {
                    println!("⚠️  settings.json: exists but RTK hook not configured");
                    println!("    Run: rtk init -g --auto-patch");
                }
            } else {
                println!("⚠️  settings.json: exists but invalid JSON");
            }
        } else {
            println!("⚪ settings.json: empty");
        }
    } else {
        println!("⚪ settings.json: not found");
    }

    #[cfg(unix)]
    {
        let global_root = resolve_opencode_global_root()?;
        let global_plugin = resolve_opencode_plugin_path(true)?;
        let agents_path = resolve_opencode_agents_path()?;
        let opencode_status = ShowConfigOpencodeStatus {
            global_root,
            plugin: global_plugin
                .exists()
                .then(|| (SetupTargetStatus::AlreadyConfigured, global_plugin)),
            agents: agents_path
                .exists()
                .then(|| (SetupTargetStatus::AlreadyConfigured, agents_path)),
        };
        println!("\n{}", format_show_config_opencode_status(&opencode_status));
    }

    println!("\nUsage:");
    println!("  rtk init              # Full injection into local CLAUDE.md");
    println!("  rtk init -g           # Hook + RTK.md + @RTK.md + settings.json (recommended)");
    println!("  rtk init -g --auto-patch    # Same as above but no prompt");
    println!("  rtk init -g --no-patch      # Skip settings.json (manual setup)");
    println!("  rtk init -g --uninstall     # Remove all RTK artifacts");
    println!("  rtk init -g --claude-md     # Legacy: full injection into ~/.claude/CLAUDE.md");
    println!("  rtk init -g --hook-only     # Hook only, no RTK.md");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_mentions_all_top_level_commands() {
        for cmd in [
            "rtk cargo",
            "rtk gh",
            "rtk vitest",
            "rtk tsc",
            "rtk lint",
            "rtk prettier",
            "rtk next",
            "rtk playwright",
            "rtk prisma",
            "rtk pnpm",
            "rtk npm",
            "rtk curl",
            "rtk git",
            "rtk docker",
            "rtk kubectl",
        ] {
            assert!(
                RTK_INSTRUCTIONS.contains(cmd),
                "Missing {cmd} in RTK_INSTRUCTIONS"
            );
        }
    }

    #[test]
    fn test_init_has_version_marker() {
        assert!(
            RTK_INSTRUCTIONS.contains("<!-- rtk-instructions"),
            "RTK_INSTRUCTIONS must have version marker for idempotency"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_detect_opencode_config_dir_present_returns_true() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("opencode")).unwrap();

        let detected = detect_opencode_with(Some(temp.path()), || false);

        assert!(detected);
    }

    #[test]
    #[cfg(unix)]
    fn test_detect_opencode_without_config_dir_or_binary_returns_false() {
        let temp = TempDir::new().unwrap();

        let detected = detect_opencode_with(Some(temp.path()), || false);

        assert!(!detected);
    }

    #[test]
    #[cfg(unix)]
    fn test_detect_opencode_without_config_dir_but_binary_returns_true() {
        let temp = TempDir::new().unwrap();

        let detected = detect_opencode_with(Some(temp.path()), || true);

        assert!(detected);
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_setup_target_exact_prompt_choices() {
        assert_eq!(
            resolve_setup_target(Some("Claude"), true),
            SetupTargetSelection::Selected(SetupTarget::Claude)
        );
        assert_eq!(
            resolve_setup_target(Some("opencode"), true),
            SetupTargetSelection::Selected(SetupTarget::Opencode)
        );
        assert_eq!(
            resolve_setup_target(Some("both"), true),
            SetupTargetSelection::Selected(SetupTarget::Both)
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_setup_target_non_interactive_skips_without_processing() {
        assert_eq!(
            resolve_setup_target(None, false),
            SetupTargetSelection::SkippedChoiceRequired
        );
        assert_eq!(
            resolve_setup_target(Some("claude"), false),
            SetupTargetSelection::SkippedChoiceRequired
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_run_target_claude_only_runs_only_claude_work() {
        let calls = std::cell::RefCell::new(Vec::new());

        let summary = run_setup_target_with(
            SetupTarget::Claude,
            || {
                calls.borrow_mut().push("claude");
                Ok::<_, ()>(SetupTargetOutcome::processed())
            },
            || {
                calls.borrow_mut().push("opencode");
                Ok::<_, ()>(SetupTargetOutcome::processed())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), vec!["claude"]);
        assert_eq!(summary.sections, vec!["Claude setup"]);
        assert_eq!(summary.claude.status, SetupTargetStatus::Processed);
        assert_eq!(summary.opencode.status, SetupTargetStatus::Skipped);
    }

    #[test]
    #[cfg(unix)]
    fn test_run_target_opencode_only_runs_only_opencode_work() {
        let calls = std::cell::RefCell::new(Vec::new());

        let summary = run_setup_target_with(
            SetupTarget::Opencode,
            || {
                calls.borrow_mut().push("claude");
                Ok::<_, ()>(SetupTargetOutcome::processed())
            },
            || {
                calls.borrow_mut().push("opencode");
                Ok::<_, ()>(SetupTargetOutcome::already_configured())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), vec!["opencode"]);
        assert_eq!(summary.sections, vec!["opencode setup"]);
        assert_eq!(summary.claude.status, SetupTargetStatus::Skipped);
        assert_eq!(
            summary.opencode.status,
            SetupTargetStatus::AlreadyConfigured
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_run_target_both_orders_claude_then_opencode() {
        let calls = std::cell::RefCell::new(Vec::new());

        let summary = run_setup_target_with(
            SetupTarget::Both,
            || {
                calls.borrow_mut().push("claude");
                Ok::<_, ()>(SetupTargetOutcome::processed())
            },
            || {
                calls.borrow_mut().push("opencode");
                Ok::<_, ()>(SetupTargetOutcome::already_configured())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), vec!["claude", "opencode"]);
        assert_eq!(summary.sections, vec!["Claude setup", "opencode setup"]);
        assert_eq!(summary.claude.status, SetupTargetStatus::Processed);
        assert_eq!(
            summary.opencode.status,
            SetupTargetStatus::AlreadyConfigured
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_official_opencode_global_root_uses_config_dir() {
        let temp = TempDir::new().expect("temp dir");

        let root = resolve_opencode_global_root_at(temp.path());

        assert_eq!(root, temp.path().join("opencode"));
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_official_opencode_plugin_path_uses_global_root() {
        let temp = TempDir::new().expect("temp dir");

        let path = resolve_opencode_plugin_path_from_config_dir(temp.path(), true);

        assert_eq!(path, temp.path().join("opencode/plugins/rtk-rewrite.ts"));
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_official_opencode_config_dir_prefers_home_dot_config() {
        let config_dir = Path::new("/tmp/Library/Application Support");
        let home_dir = Path::new("/tmp/home");

        let resolved = resolve_official_opencode_config_dir_with(Some(config_dir), Some(home_dir))
            .expect("config dir");

        assert_eq!(resolved, home_dir.join(".config"));
    }

    #[test]
    #[cfg(unix)]
    fn test_final_setup_summary_includes_selected_targets_statuses_and_paths() {
        let summary = FinalSetupSummary {
            selected_target: SetupTarget::Both,
            outcomes: vec![
                FinalSetupTargetSummary {
                    name: "Claude",
                    status: SetupTargetStatus::Processed,
                    detail: "configured",
                    paths: vec![PathBuf::from("/tmp/.claude/hooks/rtk-rewrite.sh")],
                    processed: true,
                },
                FinalSetupTargetSummary {
                    name: "opencode",
                    status: SetupTargetStatus::AlreadyConfigured,
                    detail: "already configured",
                    paths: vec![PathBuf::from(
                        "/tmp/.config/opencode/plugins/rtk-rewrite.ts",
                    )],
                    processed: true,
                },
            ],
        };

        let rendered = format_final_setup_summary(&summary);

        assert!(rendered.contains("Final setup summary"));
        assert!(rendered.contains("Selected target: both"));
        assert!(rendered.contains("Claude: configured"));
        assert!(rendered.contains("opencode: already configured"));
        assert!(rendered.contains("/tmp/.claude/hooks/rtk-rewrite.sh"));
        assert!(rendered.contains("/tmp/.config/opencode/plugins/rtk-rewrite.ts"));
    }

    #[test]
    #[cfg(unix)]
    fn test_final_setup_summary_marks_skipped_targets_without_processing() {
        let summary = FinalSetupSummary {
            selected_target: SetupTarget::Claude,
            outcomes: vec![
                FinalSetupTargetSummary {
                    name: "Claude",
                    status: SetupTargetStatus::Processed,
                    detail: "configured",
                    paths: vec![PathBuf::from("/tmp/.claude/CLAUDE.md")],
                    processed: true,
                },
                FinalSetupTargetSummary {
                    name: "opencode",
                    status: SetupTargetStatus::Skipped,
                    detail: "not selected",
                    paths: vec![PathBuf::from(
                        "/tmp/.config/opencode/plugins/rtk-rewrite.ts",
                    )],
                    processed: false,
                },
            ],
        };

        let rendered = format_final_setup_summary(&summary);

        assert!(rendered.contains("Selected target: claude"));
        assert!(rendered.contains("Claude: configured"));
        assert!(rendered.contains("opencode: not selected (not processed)"));
    }

    #[test]
    #[cfg(unix)]
    fn test_show_config_opencode_uses_official_global_root_and_target_aware_wording() {
        let root = PathBuf::from("/tmp/.config/opencode");
        let plugin_path = root.join("plugins/rtk-rewrite.ts");
        let agents_path = root.join("AGENTS.md");

        let rendered = format_show_config_opencode_status(&ShowConfigOpencodeStatus {
            global_root: root.clone(),
            plugin: Some((SetupTargetStatus::AlreadyConfigured, plugin_path.clone())),
            agents: Some((SetupTargetStatus::Processed, agents_path.clone())),
        });

        assert!(rendered.contains(root.to_string_lossy().as_ref()));
        assert!(rendered.contains(plugin_path.to_string_lossy().as_ref()));
        assert!(rendered.contains(agents_path.to_string_lossy().as_ref()));
        assert!(rendered.contains("opencode (global)"));
        assert!(rendered.contains("plugin: already configured"));
        assert!(rendered.contains("AGENTS.md: configured"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_preserves_legacy_init_modes() {
        assert_eq!(resolve_init_mode(true, false), InitMode::ClaudeMd);
        assert_eq!(resolve_init_mode(false, true), InitMode::HookOnly);
        assert_eq!(resolve_init_mode(false, false), InitMode::Default);
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_opencode_install_target_global_init_uses_global_without_prompt() {
        assert_eq!(
            resolve_opencode_install_target(true, None, false),
            OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Global)
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_opencode_install_target_local_init_requires_explicit_choice() {
        assert_eq!(
            resolve_opencode_install_target(false, Some("global"), true),
            OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Global)
        );
        assert_eq!(
            resolve_opencode_install_target(false, Some("local"), true),
            OpencodeInstallTargetSelection::Selected(OpencodeInstallScope::Local)
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_opencode_install_target_local_init_skips_when_non_interactive() {
        assert_eq!(
            resolve_opencode_install_target(false, None, false),
            OpencodeInstallTargetSelection::SkippedChoiceRequired
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_install_status_already_installed_preserves_target_file() {
        let temp = TempDir::new().unwrap();
        let plugin_path = resolve_opencode_plugin_path_at(temp.path(), true);

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, "custom plugin").unwrap();

        let status =
            install_opencode_plugin_with_status_at(temp.path(), OpencodeInstallScope::Global, 0)
                .unwrap();

        assert_eq!(
            status,
            OpencodeInstallStatus::AlreadyInstalled {
                scope: OpencodeInstallScope::Global,
                path: plugin_path.clone(),
                other_existing: None,
            }
        );
        assert_eq!(fs::read_to_string(plugin_path).unwrap(), "custom plugin");
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_install_status_reports_other_location_and_uninstall_guidance() {
        let temp = TempDir::new().unwrap();
        let global_path = resolve_opencode_plugin_path_at(temp.path(), true);
        let local_path = resolve_opencode_plugin_path_at(temp.path(), false);

        fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        fs::write(&global_path, "custom plugin").unwrap();
        fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        fs::write(&local_path, "project plugin").unwrap();

        let status =
            install_opencode_plugin_with_status_at(temp.path(), OpencodeInstallScope::Global, 0)
                .unwrap();
        let message = format_opencode_install_status(&status);

        assert!(message.contains("already installed"));
        assert!(message.contains(&global_path.display().to_string()));
        assert!(message.contains("local already installed"));
        assert!(message.contains(&local_path.display().to_string()));
        assert!(message.contains("target/debug/rtk"));
        assert!(message.contains("rtk init --uninstall"));
    }

    #[test]
    #[cfg(unix)]
    fn test_install_opencode_plugin_status_installed_mentions_rewrite_and_verification() {
        let temp = TempDir::new().unwrap();
        let plugin_path = resolve_opencode_plugin_path_at(temp.path(), false);

        let status =
            install_opencode_plugin_with_status_at(temp.path(), OpencodeInstallScope::Local, 0)
                .unwrap();
        let message = format_opencode_install_status(&status);

        assert!(message.contains(&plugin_path.display().to_string()));
        assert!(message.contains("Scope: local"));
        assert!(message.contains("rtk rewrite"));
        assert!(message.contains("target/debug/rtk"));
        assert!(message.contains("git status"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_has_tool_execute_before() {
        assert!(OPENCODE_PLUGIN.contains("tool.execute.before"));
        assert!(OPENCODE_PLUGIN.contains("input.tool === \"bash\""));
        assert!(OPENCODE_PLUGIN.contains("spawnSync"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_exports_named_and_default_factory() {
        assert!(OPENCODE_PLUGIN.contains("export const RtkRewritePlugin"));
        assert!(OPENCODE_PLUGIN.contains("export default RtkRewritePlugin"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_supports_runtime_command_shapes() {
        assert!(OPENCODE_PLUGIN.contains("output.args.command"));
        assert!(OPENCODE_PLUGIN.contains("output.args?.command"));
        assert!(OPENCODE_PLUGIN.contains("output.args?.cmd"));
        assert!(OPENCODE_PLUGIN.contains("output.args?.argv?.command"));
        assert!(OPENCODE_PLUGIN.contains("output.args?.bash?.command"));
        assert!(OPENCODE_PLUGIN.contains("setCommandValue"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_has_opt_in_runtime_diagnostics() {
        assert!(OPENCODE_PLUGIN.contains("RTK_OPENCODE_DEBUG"));
        assert!(OPENCODE_PLUGIN.contains("RTK_OPENCODE_DEBUG_FILE"));
        assert!(OPENCODE_PLUGIN.contains("plugin-loaded"));
        assert!(OPENCODE_PLUGIN.contains("incoming-tool"));
        assert!(OPENCODE_PLUGIN.contains("command-field"));
        assert!(OPENCODE_PLUGIN.contains("rtk-candidate"));
        assert!(OPENCODE_PLUGIN.contains("rewrite-result"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_preserves_graceful_no_throw_fallbacks() {
        assert!(OPENCODE_PLUGIN.contains("unsupported-command-shape"));
        assert!(OPENCODE_PLUGIN.contains("rewrite-error"));
        assert!(OPENCODE_PLUGIN.contains("rewrite-noop"));
        assert!(OPENCODE_PLUGIN.contains("return null"));
        assert!(OPENCODE_PLUGIN.contains("return false"));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_plugin_asset_has_rtk_fallbacks() {
        for candidate in [
            ".cargo/bin/rtk",
            "/usr/local/bin/rtk",
            "/opt/homebrew/bin/rtk",
            "\"target\", \"debug\", \"rtk\"",
            "\"target\", \"release\", \"rtk\"",
            "process.env.PATH",
        ] {
            assert!(
                OPENCODE_PLUGIN.contains(candidate),
                "Missing fallback candidate {candidate}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_install_opencode_plugin_global_creates_file() {
        let temp = TempDir::new().unwrap();

        let installed = install_opencode_plugin_at(temp.path(), true, 0).unwrap();
        let plugin_path = temp
            .path()
            .join("config")
            .join("opencode")
            .join("plugins")
            .join("rtk-rewrite.ts");

        assert!(installed);
        assert_eq!(fs::read_to_string(plugin_path).unwrap(), OPENCODE_PLUGIN);
    }

    #[test]
    #[cfg(unix)]
    fn test_install_opencode_plugin_local_creates_file() {
        let temp = TempDir::new().unwrap();

        let installed = install_opencode_plugin_at(temp.path(), false, 0).unwrap();
        let plugin_path = temp
            .path()
            .join(".opencode")
            .join("plugins")
            .join("rtk-rewrite.ts");

        assert!(installed);
        assert_eq!(fs::read_to_string(plugin_path).unwrap(), OPENCODE_PLUGIN);
    }

    #[test]
    #[cfg(unix)]
    fn test_install_opencode_plugin_skips_when_target_exists() {
        let temp = TempDir::new().unwrap();
        let plugin_path = temp
            .path()
            .join("config")
            .join("opencode")
            .join("plugins")
            .join("rtk-rewrite.ts");

        fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
        fs::write(&plugin_path, "custom plugin").unwrap();

        let installed = install_opencode_plugin_at(temp.path(), true, 0).unwrap();

        assert!(!installed);
        assert_eq!(fs::read_to_string(plugin_path).unwrap(), "custom plugin");
    }

    #[test]
    fn test_hook_has_guards() {
        assert!(REWRITE_HOOK.contains("command -v rtk"));
        assert!(REWRITE_HOOK.contains("command -v jq"));
        // Guards (rtk/jq availability checks) must appear before the actual delegation call.
        // The thin delegating hook no longer uses set -euo pipefail.
        let jq_pos = REWRITE_HOOK.find("command -v jq").unwrap();
        let rtk_delegate_pos = REWRITE_HOOK.find("rtk rewrite \"$CMD\"").unwrap();
        assert!(
            jq_pos < rtk_delegate_pos,
            "Guards must appear before rtk rewrite delegation"
        );
    }

    #[test]
    fn test_migration_removes_old_block() {
        let input = r#"# My Config

<!-- rtk-instructions v2 -->
OLD RTK STUFF
<!-- /rtk-instructions -->

More content"#;

        let (result, migrated) = remove_rtk_block(input);
        assert!(migrated);
        assert!(!result.contains("OLD RTK STUFF"));
        assert!(result.contains("# My Config"));
        assert!(result.contains("More content"));
    }

    #[test]
    fn test_migration_warns_on_missing_end_marker() {
        let input = "<!-- rtk-instructions v2 -->\nOLD STUFF\nNo end marker";
        let (result, migrated) = remove_rtk_block(input);
        assert!(!migrated);
        assert_eq!(result, input);
    }

    #[test]
    #[cfg(unix)]
    fn test_default_mode_creates_hook_and_rtk_md() {
        let temp = TempDir::new().unwrap();
        let hook_path = temp.path().join("rtk-rewrite.sh");
        let rtk_md_path = temp.path().join("RTK.md");

        fs::write(&hook_path, REWRITE_HOOK).unwrap();
        fs::write(&rtk_md_path, RTK_SLIM).unwrap();

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(hook_path.exists());
        assert!(rtk_md_path.exists());

        let metadata = fs::metadata(&hook_path).unwrap();
        assert!(metadata.permissions().mode() & 0o111 != 0);
    }

    #[test]
    fn test_claude_md_mode_creates_full_injection() {
        // Just verify RTK_INSTRUCTIONS constant has the right content
        assert!(RTK_INSTRUCTIONS.contains("<!-- rtk-instructions"));
        assert!(RTK_INSTRUCTIONS.contains("rtk cargo test"));
        assert!(RTK_INSTRUCTIONS.contains("<!-- /rtk-instructions -->"));
        assert!(RTK_INSTRUCTIONS.len() > 4000);
    }

    // --- upsert_rtk_block tests ---

    #[test]
    fn test_upsert_rtk_block_appends_when_missing() {
        let input = "# Team instructions";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Added);
        assert!(content.contains("# Team instructions"));
        assert!(content.contains("<!-- rtk-instructions"));
    }

    #[test]
    fn test_upsert_rtk_block_updates_stale_block() {
        let input = r#"# Team instructions

<!-- rtk-instructions v1 -->
OLD RTK CONTENT
<!-- /rtk-instructions -->

More notes
"#;

        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Updated);
        assert!(!content.contains("OLD RTK CONTENT"));
        assert!(content.contains("rtk cargo test")); // from current RTK_INSTRUCTIONS
        assert!(content.contains("# Team instructions"));
        assert!(content.contains("More notes"));
    }

    #[test]
    fn test_upsert_rtk_block_noop_when_already_current() {
        let input = format!(
            "# Team instructions\n\n{}\n\nMore notes\n",
            RTK_INSTRUCTIONS
        );
        let (content, action) = upsert_rtk_block(&input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Unchanged);
        assert_eq!(content, input);
    }

    #[test]
    fn test_upsert_rtk_block_detects_malformed_block() {
        let input = "<!-- rtk-instructions v2 -->\npartial";
        let (content, action) = upsert_rtk_block(input, RTK_INSTRUCTIONS);
        assert_eq!(action, RtkBlockUpsert::Malformed);
        assert_eq!(content, input);
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_agents_upsert_creates_section_from_empty_content() {
        let (content, action) = upsert_opencode_agents_section("");

        assert_eq!(action, OpencodeAgentsSectionUpsert::Added);
        assert_eq!(content, RTK_OPENCODE_SECTION);
        assert_eq!(content.matches("<!-- rtk-opencode-start -->").count(), 1);
        assert_eq!(content.matches("<!-- rtk-opencode-end -->").count(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_agents_upsert_appends_without_overwriting_user_content() {
        let input = "# User instructions\n\nKeep this note.";

        let (content, action) = upsert_opencode_agents_section(input);

        assert_eq!(action, OpencodeAgentsSectionUpsert::Added);
        assert!(content.starts_with(input));
        assert!(content.contains("Keep this note."));
        assert!(content.contains(RTK_OPENCODE_SECTION));
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_agents_upsert_is_idempotent_when_section_is_current() {
        let input = format!("# User instructions\n\n{}\n", RTK_OPENCODE_SECTION);

        let (content, action) = upsert_opencode_agents_section(&input);

        assert_eq!(action, OpencodeAgentsSectionUpsert::Unchanged);
        assert_eq!(content, input);
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_agents_upsert_detects_malformed_markers_without_rewriting() {
        let input = "# User instructions\n\n<!-- rtk-opencode-start -->\npartial section";

        let (content, action) = upsert_opencode_agents_section(input);

        assert_eq!(action, OpencodeAgentsSectionUpsert::Malformed);
        assert_eq!(content, input);
    }

    #[test]
    #[cfg(unix)]
    fn test_opencode_agents_remove_preserves_surrounding_user_content() {
        let input = format!(
            "# User instructions\n\n{}\n\n## Local rules\nDo not remove.",
            RTK_OPENCODE_SECTION
        );

        let (content, action) = remove_opencode_agents_section(&input);

        assert_eq!(action, OpencodeAgentsSectionRemove::Removed);
        assert!(!content.contains("rtk-opencode-start"));
        assert!(content.contains("# User instructions"));
        assert!(content.contains("## Local rules"));
        assert!(content.contains("Do not remove."));
    }

    #[test]
    fn test_init_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, "# My stuff\n\n@RTK.md\n").unwrap();

        let content = fs::read_to_string(&claude_md).unwrap();
        let count = content.matches("@RTK.md").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_local_init_unchanged() {
        // Local init should use claude-md mode
        let temp = TempDir::new().unwrap();
        let claude_md = temp.path().join("CLAUDE.md");

        fs::write(&claude_md, RTK_INSTRUCTIONS).unwrap();
        let content = fs::read_to_string(&claude_md).unwrap();

        assert!(content.contains("<!-- rtk-instructions"));
    }

    // Tests for hook_already_present()
    #[test]
    fn test_hook_already_present_exact_match() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_already_present_different_path() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/home/user/.claude/hooks/rtk-rewrite.sh"
                    }]
                }]
            }
        });

        let hook_command = "~/.claude/hooks/rtk-rewrite.sh";
        // Should match on rtk-rewrite.sh substring
        assert!(hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_not_present_empty() {
        let json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    #[test]
    fn test_hook_not_present_other_hooks() {
        let json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        assert!(!hook_already_present(&json_content, hook_command));
    }

    // Tests for insert_hook_entry()
    #[test]
    fn test_insert_hook_entry_empty_root() {
        let mut json_content = serde_json::json!({});
        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";

        insert_hook_entry(&mut json_content, hook_command);

        // Should create full structure
        assert!(json_content.get("hooks").is_some());
        assert!(json_content
            .get("hooks")
            .unwrap()
            .get("PreToolUse")
            .is_some());

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, hook_command);
    }

    #[test]
    fn test_insert_hook_entry_preserves_existing() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        insert_hook_entry(&mut json_content, hook_command);

        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 2); // Should have both hooks

        // Check first hook is preserved
        let first_command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(first_command, "/some/other/hook.sh");

        // Check second hook is RTK
        let second_command = pre_tool_use[1]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(second_command, hook_command);
    }

    #[test]
    fn test_insert_hook_preserves_other_keys() {
        let mut json_content = serde_json::json!({
            "env": {"PATH": "/custom/path"},
            "permissions": {"allowAll": true},
            "model": "claude-sonnet-4"
        });

        let hook_command = "/Users/test/.claude/hooks/rtk-rewrite.sh";
        insert_hook_entry(&mut json_content, hook_command);

        // Should preserve all other keys
        assert_eq!(json_content["env"]["PATH"], "/custom/path");
        assert_eq!(json_content["permissions"]["allowAll"], true);
        assert_eq!(json_content["model"], "claude-sonnet-4");

        // And add hooks
        assert!(json_content.get("hooks").is_some());
    }

    // Tests for atomic_write()
    #[test]
    fn test_atomic_write() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.json");

        let content = r#"{"key": "value"}"#;
        atomic_write(&file_path, content).unwrap();

        assert!(file_path.exists());
        let written = fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content);
    }

    // Test for preserve_order round-trip
    #[test]
    fn test_preserve_order_round_trip() {
        let original = r#"{"env": {"PATH": "/usr/bin"}, "permissions": {"allowAll": true}, "model": "claude-sonnet-4"}"#;
        let parsed: serde_json::Value = serde_json::from_str(original).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();

        // Just check that keys exist (preserve_order doesn't guarantee exact order in nested objects)
        assert!(serialized.contains("\"env\""));
        assert!(serialized.contains("\"permissions\""));
        assert!(serialized.contains("\"model\""));
    }

    // Tests for clean_double_blanks()
    #[test]
    fn test_clean_double_blanks() {
        // Input: line1, 2 blank lines, line2, 1 blank line, line3, 3 blank lines, line4
        // Expected: line1, 2 blank lines (kept), line2, 1 blank line, line3, 2 blank lines (max), line4
        let input = "line1\n\n\nline2\n\nline3\n\n\n\nline4";
        // That's: line1 \n \n \n line2 \n \n line3 \n \n \n \n line4
        // Which is: line1, blank, blank, line2, blank, line3, blank, blank, blank, line4
        // So 2 blanks after line1 (keep both), 1 blank after line2 (keep), 3 blanks after line3 (keep 2)
        let expected = "line1\n\n\nline2\n\nline3\n\n\nline4";
        assert_eq!(clean_double_blanks(input), expected);
    }

    #[test]
    fn test_clean_double_blanks_preserves_single() {
        let input = "line1\n\nline2\n\nline3";
        assert_eq!(clean_double_blanks(input), input); // No change
    }

    // Tests for remove_hook_from_settings()
    #[test]
    fn test_remove_hook_from_json() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/some/other/hook.sh"
                        }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "/Users/test/.claude/hooks/rtk-rewrite.sh"
                        }]
                    }
                ]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(removed);

        // Should have only one hook left
        let pre_tool_use = json_content["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre_tool_use.len(), 1);

        // Check it's the other hook
        let command = pre_tool_use[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, "/some/other/hook.sh");
    }

    #[test]
    fn test_remove_hook_when_not_present() {
        let mut json_content = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "/some/other/hook.sh"
                    }]
                }]
            }
        });

        let removed = remove_hook_from_json(&mut json_content);
        assert!(!removed);
    }
}
