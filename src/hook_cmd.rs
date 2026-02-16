use lazy_static::lazy_static;
use regex::Regex;
use std::io::Read;

lazy_static! {
    /// Matches leading env var assignments: FOO=bar BAZ=qux
    static ref ENV_PREFIX_RE: Regex =
        Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+").unwrap();

    /// Strips git global flags before subcommand: -C path, -c key=val, --no-pager, etc.
    static ref GIT_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(-C|-c)\s+\S+\s*|--[a-z-]+=\S+\s*|--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*)").unwrap();

    /// Strips docker global flags: -H, --context, --config
    static ref DOCKER_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(-H|--context|--config)\s+\S+\s*|--[a-z-]+=\S+\s*)").unwrap();

    /// Strips kubectl global flags: --context, --kubeconfig, --namespace, -n
    static ref KUBECTL_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(--context|--kubeconfig|--namespace|-n)\s+\S+\s*|--[a-z-]+=\S+\s*)").unwrap();

    /// Cargo toolchain prefix: cargo +nightly ...
    static ref CARGO_TOOLCHAIN_RE: Regex =
        Regex::new(r"^\+\S+\s+").unwrap();

    /// head -N file pattern
    static ref HEAD_DASH_N_RE: Regex =
        Regex::new(r"^head\s+-(\d+)\s+(.+)$").unwrap();

    /// head --lines=N file pattern
    static ref HEAD_LINES_RE: Regex =
        Regex::new(r"^head\s+--lines=(\d+)\s+(.+)$").unwrap();
}

/// Entry point for `rtk hook-rewrite`.
/// Reads JSON from stdin, rewrites command if it matches known patterns.
/// Never returns — always exits with code 0.
pub fn run() -> ! {
    // Read all stdin
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(0);
    }

    // Parse JSON
    let root: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => std::process::exit(0),
    };

    // Extract command
    let cmd = match root
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) if !c.is_empty() => c,
        _ => std::process::exit(0),
    };

    // Skip heredocs
    if cmd.contains("<<") {
        std::process::exit(0);
    }

    // Rewrite each segment of the command chain independently
    // (already-rtk check is done per-segment in rewrite_segment)
    let rewritten = match rewrite_chain(cmd) {
        Some(r) => r,
        None => std::process::exit(0),
    };

    // Build output JSON: preserve all original tool_input fields, override command
    let mut updated_input = match root.get("tool_input").cloned() {
        Some(v) => v,
        None => std::process::exit(0),
    };
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("command".to_string(), serde_json::Value::String(rewritten));
    }

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input
        }
    });

    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    std::process::exit(0);
}

/// Rewrite a command chain: split on " && " / " ; ", rewrite each segment.
/// Returns Some(rewritten) if at least one segment was rewritten, None otherwise.
fn rewrite_chain(cmd: &str) -> Option<String> {
    let segments = split_chain_segments(cmd);

    let mut any_rewritten = false;
    let mut result = String::with_capacity(cmd.len() + 32);

    for (i, (segment, separator)) in segments.iter().enumerate() {
        if i > 0 {
            // Write the separator from the previous segment
            if let Some(sep) = &segments[i - 1].1 {
                result.push_str(sep);
            }
        }

        // Try to rewrite this segment
        match rewrite_segment(segment) {
            Some(rewritten) => {
                result.push_str(&rewritten);
                any_rewritten = true;
            }
            None => {
                result.push_str(segment);
            }
        }
    }

    if any_rewritten {
        Some(result)
    } else {
        None
    }
}

/// Try to rewrite a single command segment (with env prefix handling).
fn rewrite_segment(segment: &str) -> Option<String> {
    let trimmed = segment.trim();
    if trimmed.is_empty() || trimmed.starts_with("rtk ") || trimmed.contains("/rtk ") {
        return None;
    }

    let (env_prefix, match_cmd, cmd_body) = strip_env_prefix(trimmed);
    try_rewrite(match_cmd, cmd_body).map(|r| {
        // Preserve leading whitespace from original segment
        let leading_ws = &segment[..segment.len() - segment.trim_start().len()];
        format!("{}{}{}", leading_ws, env_prefix, r)
    })
}

/// Split a command on " && " and " ; " separators, respecting quotes.
/// Returns Vec of (segment, Option<separator>).
fn split_chain_segments(cmd: &str) -> Vec<(&str, Option<&str>)> {
    let mut segments = Vec::new();
    let bytes = cmd.as_bytes();
    let mut start = 0;
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double_quote => in_single_quote = !in_single_quote,
            b'"' if !in_single_quote => in_double_quote = !in_double_quote,
            b' ' if !in_single_quote && !in_double_quote => {
                let rest = &cmd[i..];
                if rest.starts_with(" && ") {
                    segments.push((&cmd[start..i], Some(" && ")));
                    i += 4;
                    start = i;
                    continue;
                }
                if rest.starts_with(" ; ") {
                    segments.push((&cmd[start..i], Some(" ; ")));
                    i += 3;
                    start = i;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Last segment
    segments.push((&cmd[start..], None));
    segments
}

/// Strip leading env var assignments (e.g. "FOO=bar BAZ=1 cmd args")
/// Returns (env_prefix, match_cmd, cmd_body)
/// - env_prefix: "FOO=bar BAZ=1 " (with trailing space)
/// - match_cmd: "cmd args" (for pattern matching)
/// - cmd_body: "cmd args" (same as match_cmd, used for rewriting)
fn strip_env_prefix(cmd: &str) -> (&str, &str, &str) {
    if let Some(m) = ENV_PREFIX_RE.find(cmd) {
        let prefix = &cmd[..m.end()];
        let rest = &cmd[m.end()..];
        (prefix, rest, rest)
    } else {
        ("", cmd, cmd)
    }
}

/// Attempt to rewrite a command. Returns Some(rewritten_body) or None.
fn try_rewrite(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // --- Git ---
    if match_cmd.starts_with("git ") || match_cmd == "git" {
        return try_rewrite_git(match_cmd, cmd_body);
    }

    // --- GitHub CLI ---
    if match_cmd.starts_with("gh ") {
        return try_rewrite_gh(match_cmd, cmd_body);
    }

    // --- Cargo ---
    if match_cmd.starts_with("cargo ") {
        return try_rewrite_cargo(match_cmd, cmd_body);
    }

    // --- File operations ---
    if match_cmd.starts_with("cat ") {
        return Some(replace_prefix(cmd_body, "cat ", "rtk read "));
    }
    if match_cmd.starts_with("rg ") {
        return Some(replace_prefix(cmd_body, "rg ", "rtk grep "));
    }
    if match_cmd.starts_with("grep ") {
        return Some(replace_prefix(cmd_body, "grep ", "rtk grep "));
    }
    if match_cmd == "ls" || match_cmd.starts_with("ls ") {
        return Some(replace_prefix(cmd_body, "ls", "rtk ls"));
    }
    if match_cmd == "tree" || match_cmd.starts_with("tree ") {
        return Some(replace_prefix(cmd_body, "tree", "rtk tree"));
    }
    if match_cmd.starts_with("find ") {
        return Some(replace_prefix(cmd_body, "find ", "rtk find "));
    }
    if match_cmd.starts_with("diff ") {
        return Some(replace_prefix(cmd_body, "diff ", "rtk diff "));
    }
    if match_cmd.starts_with("head ") {
        return try_rewrite_head(match_cmd);
    }

    // --- JS/TS tooling ---
    if let Some(r) = try_rewrite_js_ts(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Containers ---
    if match_cmd.starts_with("docker ") {
        return try_rewrite_docker(match_cmd, cmd_body);
    }
    if match_cmd.starts_with("kubectl ") {
        return try_rewrite_kubectl(match_cmd, cmd_body);
    }

    // --- Network ---
    if match_cmd.starts_with("curl ") {
        return Some(replace_prefix(cmd_body, "curl ", "rtk curl "));
    }
    if match_cmd.starts_with("wget ") {
        return Some(replace_prefix(cmd_body, "wget ", "rtk wget "));
    }

    // --- pnpm package management ---
    if let Some(r) = try_rewrite_pnpm_pkg(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Python ---
    if let Some(r) = try_rewrite_python(match_cmd, cmd_body) {
        return Some(r);
    }

    // --- Go ---
    if let Some(r) = try_rewrite_go(match_cmd, cmd_body) {
        return Some(r);
    }

    None
}

// --- Category-specific rewriters ---

fn try_rewrite_git(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // Strip git global flags to find the subcommand
    let after_git = match_cmd.strip_prefix("git ").unwrap_or("");
    let stripped = GIT_GLOBAL_FLAGS_RE.replace_all(after_git, "");
    let stripped = stripped.trim_start();

    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "status" | "diff" | "log" | "add" | "commit" | "push" | "pull" | "branch" | "fetch"
        | "stash" | "show" | "worktree" => Some(format!("rtk {}", cmd_body)),
        _ => None,
    }
}

fn try_rewrite_gh(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_gh = match_cmd.strip_prefix("gh ").unwrap_or("");
    let subcmd = after_gh.split_whitespace().next().unwrap_or("");
    match subcmd {
        "pr" | "issue" | "run" | "api" | "release" => {
            Some(replace_prefix(cmd_body, "gh ", "rtk gh "))
        }
        _ => None,
    }
}

fn try_rewrite_cargo(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_cargo = match_cmd.strip_prefix("cargo ").unwrap_or("");
    // Strip toolchain prefix (+nightly, +stable, etc.)
    let after_toolchain = CARGO_TOOLCHAIN_RE.replace(after_cargo, "");
    let subcmd = after_toolchain.split_whitespace().next().unwrap_or("");
    match subcmd {
        "test" | "build" | "clippy" | "check" | "install" | "fmt" => {
            Some(format!("rtk {}", cmd_body))
        }
        _ => None,
    }
}

fn try_rewrite_head(match_cmd: &str) -> Option<String> {
    // head -N file → rtk read file --max-lines N
    if let Some(caps) = HEAD_DASH_N_RE.captures(match_cmd) {
        let lines = &caps[1];
        let file = &caps[2];
        return Some(format!("rtk read {} --max-lines {}", file, lines));
    }
    // head --lines=N file
    if let Some(caps) = HEAD_LINES_RE.captures(match_cmd) {
        let lines = &caps[1];
        let file = &caps[2];
        return Some(format!("rtk read {} --max-lines {}", file, lines));
    }
    None
}

fn try_rewrite_js_ts(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // vitest (with optional pnpm/npx prefix)
    if starts_with_any(match_cmd, &["vitest", "pnpm vitest", "npx vitest"]) {
        // Strip prefixes, replace with "rtk vitest run"
        let rest = match_cmd
            .trim_start_matches("pnpm ")
            .trim_start_matches("npx ")
            .trim_start_matches("vitest")
            .trim_start_matches(" run")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk vitest run".to_string()
        } else {
            format!("rtk vitest run {}", rest)
        });
    }

    // pnpm test → rtk vitest run
    if match_cmd == "pnpm test" || match_cmd.starts_with("pnpm test ") {
        let rest = match_cmd.strip_prefix("pnpm test").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk vitest run".to_string()
        } else {
            format!("rtk vitest run {}", rest)
        });
    }

    // npm test
    if match_cmd == "npm test" || match_cmd.starts_with("npm test ") {
        return Some(replace_prefix(cmd_body, "npm test", "rtk npm test"));
    }

    // npm run
    if match_cmd.starts_with("npm run ") {
        return Some(replace_prefix(cmd_body, "npm run ", "rtk npm "));
    }

    // vue-tsc (with optional npx prefix)
    if match_cmd == "vue-tsc"
        || match_cmd.starts_with("vue-tsc ")
        || match_cmd == "npx vue-tsc"
        || match_cmd.starts_with("npx vue-tsc ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("vue-tsc")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // pnpm tsc
    if match_cmd == "pnpm tsc" || match_cmd.starts_with("pnpm tsc ") {
        let rest = match_cmd.strip_prefix("pnpm tsc").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // tsc (with optional npx prefix)
    if match_cmd == "tsc"
        || match_cmd.starts_with("tsc ")
        || match_cmd == "npx tsc"
        || match_cmd.starts_with("npx tsc ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("tsc")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // pnpm lint
    if match_cmd == "pnpm lint" || match_cmd.starts_with("pnpm lint ") {
        let rest = match_cmd.strip_prefix("pnpm lint").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk lint".to_string()
        } else {
            format!("rtk lint {}", rest)
        });
    }

    // eslint (with optional npx prefix)
    if match_cmd == "eslint"
        || match_cmd.starts_with("eslint ")
        || match_cmd == "npx eslint"
        || match_cmd.starts_with("npx eslint ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("eslint")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk lint".to_string()
        } else {
            format!("rtk lint {}", rest)
        });
    }

    // prettier (with optional npx prefix)
    if match_cmd == "prettier"
        || match_cmd.starts_with("prettier ")
        || match_cmd == "npx prettier"
        || match_cmd.starts_with("npx prettier ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("prettier")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk prettier".to_string()
        } else {
            format!("rtk prettier {}", rest)
        });
    }

    // playwright (with optional npx/pnpm prefix)
    if match_cmd == "playwright"
        || match_cmd.starts_with("playwright ")
        || match_cmd == "npx playwright"
        || match_cmd.starts_with("npx playwright ")
        || match_cmd == "pnpm playwright"
        || match_cmd.starts_with("pnpm playwright ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("pnpm ")
            .trim_start_matches("playwright")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk playwright".to_string()
        } else {
            format!("rtk playwright {}", rest)
        });
    }

    // prisma (with optional npx prefix)
    if match_cmd == "prisma"
        || match_cmd.starts_with("prisma ")
        || match_cmd == "npx prisma"
        || match_cmd.starts_with("npx prisma ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("prisma")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk prisma".to_string()
        } else {
            format!("rtk prisma {}", rest)
        });
    }

    None
}

fn try_rewrite_docker(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_docker = match_cmd.strip_prefix("docker ").unwrap_or("");

    // docker compose → always rewrite
    if after_docker.starts_with("compose") {
        return Some(replace_prefix(cmd_body, "docker ", "rtk docker "));
    }

    // Strip docker global flags
    let stripped = DOCKER_GLOBAL_FLAGS_RE.replace_all(after_docker, "");
    let stripped = stripped.trim_start();
    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "ps" | "images" | "logs" | "run" | "build" | "exec" => {
            Some(replace_prefix(cmd_body, "docker ", "rtk docker "))
        }
        _ => None,
    }
}

fn try_rewrite_kubectl(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_kubectl = match_cmd.strip_prefix("kubectl ").unwrap_or("");
    let stripped = KUBECTL_GLOBAL_FLAGS_RE.replace_all(after_kubectl, "");
    let stripped = stripped.trim_start();
    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "get" | "logs" | "describe" | "apply" => {
            Some(replace_prefix(cmd_body, "kubectl ", "rtk kubectl "))
        }
        _ => None,
    }
}

fn try_rewrite_pnpm_pkg(match_cmd: &str, cmd_body: &str) -> Option<String> {
    if !match_cmd.starts_with("pnpm ") {
        return None;
    }
    let after_pnpm = match_cmd.strip_prefix("pnpm ").unwrap_or("");
    let subcmd = after_pnpm.split_whitespace().next().unwrap_or("");
    match subcmd {
        "list" | "ls" | "outdated" => Some(replace_prefix(cmd_body, "pnpm ", "rtk pnpm ")),
        _ => None,
    }
}

fn try_rewrite_python(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // pytest
    if match_cmd == "pytest" || match_cmd.starts_with("pytest ") {
        return Some(replace_prefix(cmd_body, "pytest", "rtk pytest"));
    }

    // python -m pytest
    if match_cmd.starts_with("python -m pytest") {
        let rest = match_cmd
            .strip_prefix("python -m pytest")
            .unwrap_or("")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk pytest".to_string()
        } else {
            format!("rtk pytest {}", rest)
        });
    }

    // ruff check/format
    if match_cmd.starts_with("ruff ") {
        let after_ruff = match_cmd.strip_prefix("ruff ").unwrap_or("");
        let subcmd = after_ruff.split_whitespace().next().unwrap_or("");
        if subcmd == "check" || subcmd == "format" {
            return Some(replace_prefix(cmd_body, "ruff ", "rtk ruff "));
        }
    }

    // pip list/outdated/install/show
    if match_cmd.starts_with("pip ") {
        let after_pip = match_cmd.strip_prefix("pip ").unwrap_or("");
        let subcmd = after_pip.split_whitespace().next().unwrap_or("");
        if matches!(subcmd, "list" | "outdated" | "install" | "show") {
            return Some(replace_prefix(cmd_body, "pip ", "rtk pip "));
        }
    }

    // uv pip list/outdated/install/show
    if match_cmd.starts_with("uv pip ") {
        let after_uv_pip = match_cmd.strip_prefix("uv pip ").unwrap_or("");
        let subcmd = after_uv_pip.split_whitespace().next().unwrap_or("");
        if matches!(subcmd, "list" | "outdated" | "install" | "show") {
            return Some(replace_prefix(cmd_body, "uv pip ", "rtk pip "));
        }
    }

    None
}

fn try_rewrite_go(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // go test/build/vet
    if match_cmd.starts_with("go ") {
        let after_go = match_cmd.strip_prefix("go ").unwrap_or("");
        let subcmd = after_go.split_whitespace().next().unwrap_or("");
        match subcmd {
            "test" => return Some(replace_prefix(cmd_body, "go test", "rtk go test")),
            "build" => return Some(replace_prefix(cmd_body, "go build", "rtk go build")),
            "vet" => return Some(replace_prefix(cmd_body, "go vet", "rtk go vet")),
            _ => {}
        }
    }

    // golangci-lint
    if match_cmd == "golangci-lint" || match_cmd.starts_with("golangci-lint ") {
        return Some(replace_prefix(
            cmd_body,
            "golangci-lint",
            "rtk golangci-lint",
        ));
    }

    None
}

// --- Helpers ---

/// Replace a prefix in cmd_body. Simple string replacement of first occurrence.
fn replace_prefix(cmd_body: &str, old_prefix: &str, new_prefix: &str) -> String {
    if let Some(rest) = cmd_body.strip_prefix(old_prefix) {
        format!("{}{}", new_prefix, rest)
    } else {
        // Fallback: just prepend rtk
        format!("rtk {}", cmd_body)
    }
}

/// Check if s starts with any of the given prefixes (exact or followed by space)
fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|p| s == *p || s.starts_with(&format!("{} ", p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: simulate rewrite logic without stdin/stdout
    fn rewrite(cmd: &str) -> Option<String> {
        if cmd.contains("<<") {
            return None;
        }
        rewrite_chain(cmd)
    }

    // --- Git ---
    #[test]
    fn test_git_status() {
        assert_eq!(rewrite("git status"), Some("rtk git status".into()));
    }

    #[test]
    fn test_git_diff_cached() {
        assert_eq!(
            rewrite("git diff --cached"),
            Some("rtk git diff --cached".into())
        );
    }

    #[test]
    fn test_git_log_with_flags() {
        assert_eq!(
            rewrite("git log --oneline -10"),
            Some("rtk git log --oneline -10".into())
        );
    }

    #[test]
    fn test_git_with_global_flags() {
        assert_eq!(
            rewrite("git --no-pager diff"),
            Some("rtk git --no-pager diff".into())
        );
    }

    #[test]
    fn test_git_add() {
        assert_eq!(rewrite("git add ."), Some("rtk git add .".into()));
    }

    #[test]
    fn test_git_commit() {
        assert_eq!(
            rewrite("git commit -m \"msg\""),
            Some("rtk git commit -m \"msg\"".into())
        );
    }

    #[test]
    fn test_git_push() {
        assert_eq!(rewrite("git push"), Some("rtk git push".into()));
    }

    #[test]
    fn test_git_checkout_no_match() {
        assert_eq!(rewrite("git checkout main"), None);
    }

    // --- GitHub CLI ---
    #[test]
    fn test_gh_pr_view() {
        assert_eq!(rewrite("gh pr view 123"), Some("rtk gh pr view 123".into()));
    }

    #[test]
    fn test_gh_issue_list() {
        assert_eq!(rewrite("gh issue list"), Some("rtk gh issue list".into()));
    }

    #[test]
    fn test_gh_repo_no_match() {
        assert_eq!(rewrite("gh repo clone foo"), None);
    }

    // --- Cargo ---
    #[test]
    fn test_cargo_test() {
        assert_eq!(rewrite("cargo test"), Some("rtk cargo test".into()));
    }

    #[test]
    fn test_cargo_build_release() {
        assert_eq!(
            rewrite("cargo build --release"),
            Some("rtk cargo build --release".into())
        );
    }

    #[test]
    fn test_cargo_with_toolchain() {
        assert_eq!(
            rewrite("cargo +nightly build"),
            Some("rtk cargo +nightly build".into())
        );
    }

    #[test]
    fn test_cargo_run_no_match() {
        assert_eq!(rewrite("cargo run"), None);
    }

    // --- File operations ---
    #[test]
    fn test_cat_to_read() {
        assert_eq!(
            rewrite("cat src/main.rs"),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rg_to_grep() {
        assert_eq!(
            rewrite("rg pattern src/"),
            Some("rtk grep pattern src/".into())
        );
    }

    #[test]
    fn test_grep_to_rtk_grep() {
        assert_eq!(rewrite("grep -r TODO ."), Some("rtk grep -r TODO .".into()));
    }

    #[test]
    fn test_ls() {
        assert_eq!(rewrite("ls -la"), Some("rtk ls -la".into()));
    }

    #[test]
    fn test_ls_bare() {
        assert_eq!(rewrite("ls"), Some("rtk ls".into()));
    }

    #[test]
    fn test_find() {
        assert_eq!(
            rewrite("find . -name '*.rs'"),
            Some("rtk find . -name '*.rs'".into())
        );
    }

    #[test]
    fn test_head_dash_n() {
        assert_eq!(
            rewrite("head -20 src/main.rs"),
            Some("rtk read src/main.rs --max-lines 20".into())
        );
    }

    #[test]
    fn test_head_lines_eq() {
        assert_eq!(
            rewrite("head --lines=50 README.md"),
            Some("rtk read README.md --max-lines 50".into())
        );
    }

    // --- JS/TS ---
    #[test]
    fn test_vitest() {
        assert_eq!(rewrite("vitest run"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_npx_vitest() {
        assert_eq!(rewrite("npx vitest"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_pnpm_test() {
        assert_eq!(rewrite("pnpm test"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_npm_test() {
        assert_eq!(rewrite("npm test"), Some("rtk npm test".into()));
    }

    #[test]
    fn test_npm_run() {
        assert_eq!(rewrite("npm run build"), Some("rtk npm build".into()));
    }

    #[test]
    fn test_tsc() {
        assert_eq!(rewrite("tsc --noEmit"), Some("rtk tsc --noEmit".into()));
    }

    #[test]
    fn test_npx_tsc() {
        assert_eq!(rewrite("npx tsc --noEmit"), Some("rtk tsc --noEmit".into()));
    }

    #[test]
    fn test_eslint() {
        assert_eq!(rewrite("eslint src/"), Some("rtk lint src/".into()));
    }

    #[test]
    fn test_prettier() {
        assert_eq!(
            rewrite("prettier --check ."),
            Some("rtk prettier --check .".into())
        );
    }

    #[test]
    fn test_playwright() {
        assert_eq!(
            rewrite("npx playwright test"),
            Some("rtk playwright test".into())
        );
    }

    #[test]
    fn test_prisma() {
        assert_eq!(
            rewrite("npx prisma generate"),
            Some("rtk prisma generate".into())
        );
    }

    // --- Containers ---
    #[test]
    fn test_docker_ps() {
        assert_eq!(rewrite("docker ps"), Some("rtk docker ps".into()));
    }

    #[test]
    fn test_docker_compose() {
        assert_eq!(
            rewrite("docker compose up -d"),
            Some("rtk docker compose up -d".into())
        );
    }

    #[test]
    fn test_kubectl_get() {
        assert_eq!(
            rewrite("kubectl get pods"),
            Some("rtk kubectl get pods".into())
        );
    }

    // --- Network ---
    #[test]
    fn test_curl() {
        assert_eq!(
            rewrite("curl https://api.example.com"),
            Some("rtk curl https://api.example.com".into())
        );
    }

    #[test]
    fn test_wget() {
        assert_eq!(
            rewrite("wget https://example.com/file"),
            Some("rtk wget https://example.com/file".into())
        );
    }

    // --- pnpm package management ---
    #[test]
    fn test_pnpm_list() {
        assert_eq!(rewrite("pnpm list"), Some("rtk pnpm list".into()));
    }

    #[test]
    fn test_pnpm_outdated() {
        assert_eq!(rewrite("pnpm outdated"), Some("rtk pnpm outdated".into()));
    }

    // --- Python ---
    #[test]
    fn test_pytest() {
        assert_eq!(rewrite("pytest -x"), Some("rtk pytest -x".into()));
    }

    #[test]
    fn test_python_m_pytest() {
        assert_eq!(
            rewrite("python -m pytest tests/"),
            Some("rtk pytest tests/".into())
        );
    }

    #[test]
    fn test_ruff_check() {
        assert_eq!(
            rewrite("ruff check src/"),
            Some("rtk ruff check src/".into())
        );
    }

    #[test]
    fn test_pip_list() {
        assert_eq!(rewrite("pip list"), Some("rtk pip list".into()));
    }

    #[test]
    fn test_uv_pip_install() {
        assert_eq!(
            rewrite("uv pip install flask"),
            Some("rtk pip install flask".into())
        );
    }

    // --- Go ---
    #[test]
    fn test_go_test() {
        assert_eq!(rewrite("go test ./..."), Some("rtk go test ./...".into()));
    }

    #[test]
    fn test_go_build() {
        assert_eq!(rewrite("go build"), Some("rtk go build".into()));
    }

    #[test]
    fn test_go_vet() {
        assert_eq!(rewrite("go vet ./..."), Some("rtk go vet ./...".into()));
    }

    #[test]
    fn test_golangci_lint() {
        assert_eq!(
            rewrite("golangci-lint run"),
            Some("rtk golangci-lint run".into())
        );
    }

    // --- Edge cases ---
    #[test]
    fn test_already_rtk() {
        assert_eq!(rewrite("rtk git status"), None);
    }

    #[test]
    fn test_heredoc_skip() {
        assert_eq!(rewrite("cat <<EOF\nhello\nEOF"), None);
    }

    #[test]
    fn test_no_match() {
        assert_eq!(rewrite("terraform plan"), None);
    }

    #[test]
    fn test_env_prefix_preserved() {
        assert_eq!(
            rewrite("TEST_SESSION_ID=2 npx playwright test"),
            Some("TEST_SESSION_ID=2 rtk playwright test".into())
        );
    }

    #[test]
    fn test_env_prefix_git() {
        assert_eq!(
            rewrite("GIT_AUTHOR_NAME=test git commit -m \"msg\""),
            Some("GIT_AUTHOR_NAME=test rtk git commit -m \"msg\"".into())
        );
    }

    #[test]
    fn test_empty_command() {
        assert_eq!(rewrite(""), None);
    }

    // --- Command chains ---
    #[test]
    fn test_cd_and_git_status() {
        assert_eq!(
            rewrite("cd /c/Users/Ayoub/project && git status"),
            Some("cd /c/Users/Ayoub/project && rtk git status".into())
        );
    }

    #[test]
    fn test_cd_quoted_path() {
        assert_eq!(
            rewrite("cd \"/path with spaces\" && git diff"),
            Some("cd \"/path with spaces\" && rtk git diff".into())
        );
    }

    #[test]
    fn test_cd_semicolon() {
        assert_eq!(
            rewrite("cd /tmp ; git log --oneline"),
            Some("cd /tmp ; rtk git log --oneline".into())
        );
    }

    #[test]
    fn test_cd_no_match() {
        assert_eq!(rewrite("cd /tmp && terraform plan"), None);
    }

    #[test]
    fn test_cd_alone() {
        assert_eq!(rewrite("cd /some/path"), None);
    }

    #[test]
    fn test_chain_cargo_quality_gate() {
        assert_eq!(
            rewrite("cargo fmt --all && cargo clippy --all-targets && cargo test --all"),
            Some(
                "rtk cargo fmt --all && rtk cargo clippy --all-targets && rtk cargo test --all"
                    .into()
            )
        );
    }

    #[test]
    fn test_chain_git_add_commit_push() {
        assert_eq!(
            rewrite("git add . && git commit -m \"msg\" && git push"),
            Some("rtk git add . && rtk git commit -m \"msg\" && rtk git push".into())
        );
    }

    #[test]
    fn test_chain_cd_then_multi_commands() {
        assert_eq!(
            rewrite("cd /project && cargo fmt && cargo test"),
            Some("cd /project && rtk cargo fmt && rtk cargo test".into())
        );
    }

    #[test]
    fn test_chain_mixed_match_no_match() {
        // mkdir doesn't match, but cargo test does
        assert_eq!(
            rewrite("mkdir -p build && cargo test"),
            Some("mkdir -p build && rtk cargo test".into())
        );
    }

    #[test]
    fn test_chain_all_no_match() {
        assert_eq!(rewrite("mkdir -p build && cd build && cmake .."), None);
    }

    #[test]
    fn test_chain_with_env_prefix() {
        assert_eq!(
            rewrite("cd /project && NODE_ENV=test npm test"),
            Some("cd /project && NODE_ENV=test rtk npm test".into())
        );
    }

    #[test]
    fn test_chain_semicolon_multi() {
        assert_eq!(
            rewrite("git add . ; git status"),
            Some("rtk git add . ; rtk git status".into())
        );
    }

    // --- JSON roundtrip test ---
    #[test]
    fn test_json_roundtrip() {
        let input = r#"{"tool_input":{"command":"git status","timeout":30000}}"#;
        let root: serde_json::Value = serde_json::from_str(input).unwrap();
        let cmd = root["tool_input"]["command"].as_str().unwrap();
        assert_eq!(cmd, "git status");

        // Simulate rewrite
        let rewritten = rewrite(cmd).unwrap();
        assert_eq!(rewritten, "rtk git status");

        // Build output
        let mut updated_input = root["tool_input"].clone();
        updated_input["command"] = serde_json::Value::String(rewritten);

        // Verify timeout is preserved
        assert_eq!(updated_input["timeout"], 30000);
    }
}
