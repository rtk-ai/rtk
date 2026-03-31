pub(crate) mod claude;

use super::{analysis, lexer};

#[derive(Debug, Clone)]
pub enum HookResult {
    Rewrite(String),
    Blocked(String),
}

const MAX_REWRITE_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum HookResponse {
    NoOpinion,
    Allow(String),
    Deny(String, String),
}

pub fn check_for_hook(raw: &str, _agent: &str) -> HookResult {
    check_for_hook_inner(raw, 0)
}

fn check_for_hook_inner(raw: &str, depth: usize) -> HookResult {
    if depth >= MAX_REWRITE_DEPTH {
        return HookResult::Blocked("Rewrite loop detected (max depth exceeded)".to_string());
    }
    if raw.trim().is_empty() {
        return HookResult::Rewrite(raw.to_string());
    }

    let tokens = lexer::tokenize(raw);

    let (core_tokens, suffix) = analysis::split_safe_suffix(tokens);

    if analysis::needs_shell(&core_tokens) {
        return HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)));
    }

    match analysis::parse_chain(core_tokens) {
        Ok(commands) => {
            if commands.len() == 1 {
                let routed = if suffix.is_empty() {
                    try_route_native_command(&commands[0], raw)
                } else {
                    let core_raw = if commands[0].args.is_empty() {
                        commands[0].binary.clone()
                    } else {
                        format!("{} {}", commands[0].binary, commands[0].args.join(" "))
                    };
                    try_route_native_command(&commands[0], &core_raw)
                };

                match routed {
                    Some(rtk_cmd) => {
                        if suffix.is_empty() {
                            HookResult::Rewrite(rtk_cmd)
                        } else {
                            HookResult::Rewrite(format!("{} {}", rtk_cmd, suffix))
                        }
                    }
                    None => HookResult::Rewrite(raw.to_string()),
                }
            } else {
                let substituted = reconstruct_with_rtk(&commands);
                let inner = if suffix.is_empty() {
                    substituted
                } else {
                    format!("{} {}", substituted, suffix)
                };
                HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(&inner)))
            }
        }
        Err(_) => HookResult::Rewrite(raw.to_string()),
    }
}

pub fn is_hook_disabled() -> bool {
    std::env::var("RTK_HOOK_ENABLED").as_deref() == Ok("0") || std::env::var("RTK_ACTIVE").is_ok()
}

pub fn should_passthrough(cmd: &str) -> bool {
    if cmd.starts_with("rtk ") || cmd.contains("/rtk ") || cmd.contains("<<") {
        return true;
    }
    // ISSUE #196: gh --json/--jq/--template produces structured output that rtk gh
    // would corrupt. Pass through unchanged so callers get raw JSON.
    if (cmd.starts_with("gh ") || cmd.contains(" gh "))
        && (cmd.contains("--json") || cmd.contains("--jq") || cmd.contains("--template"))
    {
        return true;
    }
    false
}

pub fn update_command_in_tool_input(
    tool_input: Option<serde_json::Value>,
    new_cmd: String,
) -> serde_json::Value {
    use serde_json::Value;
    let mut updated = tool_input.unwrap_or_else(|| Value::Object(Default::default()));
    if let Some(obj) = updated.as_object_mut() {
        obj.insert("command".into(), Value::String(new_cmd));
    }
    updated
}

#[cfg(test)]
const FORMAT_PRESERVING: &[&str] = &["tail", "echo", "cat", "find", "fd"];

#[cfg(test)]
const TRANSPARENT_SINKS: &[&str] = &["tee", "head", "tail", "cat"];

fn escape_quotes(s: &str) -> String {
    s.replace("'", "'\\''")
}

fn is_env_assign(s: &str) -> bool {
    if let Some(eq_pos) = s.find('=') {
        let key = &s[..eq_pos];
        !key.is_empty()
            && key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    } else {
        false
    }
}

fn replace_first_word(raw: &str, old_prefix: &str, new_prefix: &str) -> String {
    raw.strip_prefix(old_prefix)
        .map(|rest| format!("{new_prefix}{rest}"))
        .unwrap_or_else(|| format!("rtk run -c '{}'", escape_quotes(raw)))
}

fn route_pnpm(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" | "ls" | "outdated" | "install" => format!("rtk {raw}"),

        // ISSUE #112: shell script sed bug produces "rtk vitest run run --coverage"
        "vitest" => {
            let after_vitest: Vec<&str> = cmd.args[1..]
                .iter()
                .map(String::as_str)
                .skip_while(|&a| a == "run")
                .collect();
            if after_vitest.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_vitest.join(" "))
            }
        }

        "test" => {
            let after_test: Vec<&str> = cmd.args[1..].iter().map(String::as_str).collect();
            if after_test.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_test.join(" "))
            }
        }

        "tsc" => replace_first_word(raw, "pnpm tsc", "rtk tsc"),
        "lint" => replace_first_word(raw, "pnpm lint", "rtk lint"),
        "eslint" => replace_first_word(raw, "pnpm eslint", "rtk lint"),
        "playwright" => replace_first_word(raw, "pnpm playwright", "rtk playwright"),

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

fn route_npx(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "tsc" | "typescript" => replace_first_word(raw, &format!("npx {sub}"), "rtk tsc"),
        "eslint" => replace_first_word(raw, "npx eslint", "rtk lint"),
        "prettier" => replace_first_word(raw, "npx prettier", "rtk prettier"),
        "playwright" => replace_first_word(raw, "npx playwright", "rtk playwright"),
        "prisma" => replace_first_word(raw, "npx prisma", "rtk prisma"),

        "vitest" => {
            let after_vitest: Vec<&str> = cmd.args[1..]
                .iter()
                .map(String::as_str)
                .skip_while(|&a| a == "run")
                .collect();
            if after_vitest.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_vitest.join(" "))
            }
        }

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

fn hook_lookup<'a>(binary: &'a str, sub: &str) -> Option<(&'static str, &'a str)> {
    let base = binary.rsplit('/').next().unwrap_or(binary);
    match base {
        "git" => match sub {
            "status" | "log" | "diff" | "show" | "add" | "commit" | "push" | "pull" | "fetch"
            | "stash" | "branch" | "worktree" => Some(("rtk git", binary)),
            _ => None,
        },
        "gh" => match sub {
            "pr" | "issue" | "run" => Some(("rtk gh", binary)),
            _ => None,
        },
        "cargo" => match sub {
            "test" | "build" | "clippy" | "check" | "install" | "fmt" => {
                Some(("rtk cargo", binary))
            }
            _ => None,
        },
        "docker" => match sub {
            "ps" | "images" | "logs" => Some(("rtk docker", binary)),
            _ => None,
        },
        "kubectl" => match sub {
            "get" | "logs" => Some(("rtk kubectl", binary)),
            _ => None,
        },
        "go" => match sub {
            "test" | "build" | "vet" => Some(("rtk go", binary)),
            _ => None,
        },
        "ruff" => match sub {
            "check" | "format" => Some(("rtk ruff", binary)),
            _ => None,
        },
        "pip" | "pip3" => match sub {
            "list" | "outdated" | "install" | "show" => Some(("rtk pip", binary)),
            _ => None,
        },
        "grep" => Some(("rtk grep", binary)),
        "rg" => Some(("rtk grep", binary)),
        "ls" => Some(("rtk ls", binary)),
        "eslint" => Some(("rtk lint", binary)),
        "biome" => Some(("rtk lint", binary)),
        "tsc" => Some(("rtk tsc", binary)),
        "prettier" => Some(("rtk prettier", binary)),
        "golangci-lint" | "golangci" => Some(("rtk golangci-lint", binary)),
        "mypy" => Some(("rtk mypy", binary)),
        "playwright" => Some(("rtk playwright", binary)),
        "prisma" => Some(("rtk prisma", binary)),
        "curl" => Some(("rtk curl", binary)),
        "pytest" => Some(("rtk pytest", binary)),
        "wc" => Some(("rtk wc", binary)),
        "gt" => Some(("rtk gt", binary)),
        "wget" | "diff" | "tree" | "find" => None,
        _ => None,
    }
}

fn is_shell_prefix_builtin(token: &str) -> bool {
    matches!(
        token,
        "noglob" | "command" | "builtin" | "exec" | "nocorrect"
    )
}

pub(crate) fn route_native_command(cmd: &analysis::NativeCommand, raw: &str) -> String {
    if is_shell_prefix_builtin(&cmd.binary) {
        if let Some(real_binary) = cmd.args.first() {
            let prefix = &cmd.binary;
            let real_args = cmd.args[1..].to_vec();
            let real_cmd = analysis::NativeCommand {
                binary: real_binary.clone(),
                args: real_args,
                operator: cmd.operator.clone(),
            };
            let core_raw = raw
                .strip_prefix(prefix)
                .map(|s| s.trim_start())
                .unwrap_or(raw);
            return match try_route_native_command(&real_cmd, core_raw) {
                Some(routed) => format!("{} {}", prefix, routed),
                None => raw.to_string(),
            };
        }
        return raw.to_string();
    }

    if is_env_assign(&cmd.binary) {
        let mut env_parts: Vec<&str> = vec![cmd.binary.as_str()];
        let mut arg_idx = 0;
        while arg_idx < cmd.args.len() && is_env_assign(&cmd.args[arg_idx]) {
            env_parts.push(&cmd.args[arg_idx]);
            arg_idx += 1;
        }
        if arg_idx < cmd.args.len() {
            let env_prefix_str = env_parts.join(" ");
            let core_raw = raw
                .strip_prefix(&env_prefix_str)
                .map(|s| s.trim_start())
                .unwrap_or_else(|| {
                    let skip = env_prefix_str.len();
                    if skip < raw.len() {
                        raw[skip..].trim_start()
                    } else {
                        raw
                    }
                });
            let real_binary = cmd.args[arg_idx].clone();
            let real_args = cmd.args[arg_idx + 1..].to_vec();
            let real_cmd = analysis::NativeCommand {
                binary: real_binary,
                args: real_args,
                operator: cmd.operator.clone(),
            };
            return match try_route_native_command(&real_cmd, core_raw) {
                Some(routed) => format!("{} {}", env_prefix_str, routed),
                None => raw.to_string(),
            };
        }
    }

    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    let sub2 = cmd.args.get(1).map(String::as_str).unwrap_or("");

    if let Some((rtk_full, prefix)) = hook_lookup(&cmd.binary, sub) {
        return replace_first_word(raw, prefix, rtk_full);
    }

    if cmd.binary == "cat" {
        return replace_first_word(raw, "cat", "rtk read");
    }

    match cmd.binary.as_str() {
        "vitest" if sub.is_empty() => "rtk vitest run".to_string(),
        "vitest" => format!("rtk {raw}"),

        "uv" if sub == "pip" && matches!(sub2, "list" | "outdated" | "install" | "show") => {
            replace_first_word(raw, "uv pip", "rtk pip")
        }

        "python" | "python3" if sub == "-m" && sub2 == "pytest" => {
            let prefix = format!("{} -m pytest", cmd.binary);
            replace_first_word(raw, &prefix, "rtk pytest")
        }

        "python" | "python3" if sub == "-m" && sub2 == "mypy" => {
            let prefix = format!("{} -m mypy", cmd.binary);
            replace_first_word(raw, &prefix, "rtk mypy")
        }

        "pnpm" => route_pnpm(cmd, raw),
        "npx" => route_npx(cmd, raw),

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

pub(crate) fn try_route_native_command(cmd: &analysis::NativeCommand, raw: &str) -> Option<String> {
    let routed = route_native_command(cmd, raw);
    if routed.starts_with("rtk run -c") {
        None
    } else {
        Some(routed)
    }
}

fn reconstruct_with_rtk(commands: &[analysis::NativeCommand]) -> String {
    commands
        .iter()
        .map(|cmd| {
            let core_raw = if cmd.args.is_empty() {
                cmd.binary.clone()
            } else {
                format!("{} {}", cmd.binary, cmd.args.join(" "))
            };

            let part = match try_route_native_command(cmd, &core_raw) {
                Some(routed) => routed,
                None => core_raw,
            };

            match &cmd.operator {
                Some(op) => format!("{} {}", part, op),
                None => part,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn format_for_claude(result: HookResult) -> (String, bool, i32) {
    match result {
        HookResult::Rewrite(cmd) => (cmd, true, 0),
        HookResult::Blocked(msg) => (msg, false, 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rewrite(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains(contains),
                "'{}' rewrite should contain '{}', got '{}'",
                input,
                contains,
                cmd
            ),
            other => panic!("Expected Rewrite for '{}', got {:?}", input, other),
        }
    }

    fn assert_blocked(input: &str, depth: usize, contains: &str) {
        match check_for_hook_inner(input, depth) {
            HookResult::Blocked(msg) => assert!(
                msg.contains(contains),
                "'{}' block msg should contain '{}', got '{}'",
                input,
                contains,
                msg
            ),
            other => panic!("Expected Blocked for '{}', got {:?}", input, other),
        }
    }

    fn assert_passthrough(input: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c"),
                    "command should NOT be wrapped in rtk run -c, got '{}'",
                    cmd
                );
                assert_eq!(cmd, input, "unknown command should pass through unchanged");
            }
            HookResult::Blocked(_) => panic!("Expected passthrough for '{}', got Blocked", input),
        }
    }

    #[test]
    fn test_escape_quotes() {
        assert_eq!(escape_quotes("hello"), "hello");
        assert_eq!(escape_quotes("it's"), "it'\\''s");
        assert_eq!(escape_quotes("it's a test's"), "it'\\''s a test'\\''s");
    }

    #[test]
    fn test_check_empty_and_whitespace() {
        match check_for_hook("", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.is_empty()),
            _ => panic!("Expected Rewrite for empty"),
        }
        match check_for_hook("   ", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.trim().is_empty()),
            _ => panic!("Expected Rewrite for whitespace"),
        }
    }

    #[test]
    fn test_safe_commands_rewrite() {
        assert_rewrite("git status", "rtk git status");
        assert_rewrite(r#"git commit -m "Fix && Bug""#, "rtk git commit");

        let shell_cases = [
            ("ls *.rs", "rtk run"),
            ("echo `date`", "rtk run"),
            ("echo $(date)", "rtk run"),
            ("echo {a,b}.txt", "rtk run"),
            ("cd /tmp && git status", "rtk run"),
        ];
        for (input, expected) in shell_cases {
            assert_rewrite(input, expected);
        }

        assert_passthrough("FOO=bar echo hello");
        assert_passthrough("echo 'hello!@#$%^&*()'");
        assert_passthrough(&format!("echo {}", "a".repeat(1000)));

        match check_for_hook("cd /tmp && git status", "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains("&&"),
                "Chain rewrite must preserve '&&', got '{}'",
                cmd
            ),
            other => panic!("Expected Rewrite for chain, got {:?}", other),
        }
    }

    #[test]
    fn test_env_prefix_routes_to_rtk_subcommand() {
        let cases = [
            ("GIT_PAGER=cat git status", "rtk git", "GIT_PAGER=cat"),
            (
                "GIT_PAGER=cat git log --oneline -10",
                "rtk git",
                "GIT_PAGER=cat",
            ),
            ("RUST_LOG=debug cargo test", "rtk cargo", "RUST_LOG=debug"),
            ("LANG=C ls -la", "rtk ls", "LANG=C"),
            (
                "TEST_SESSION_ID=2 npx playwright test --config=foo",
                "rtk playwright",
                "TEST_SESSION_ID=2",
            ),
        ];
        for (input, rtk_sub, env_prefix) in cases {
            match check_for_hook(input, "claude") {
                HookResult::Rewrite(cmd) => {
                    assert!(
                        cmd.contains(rtk_sub),
                        "'{input}' must route to '{rtk_sub}', got '{cmd}'"
                    );
                    assert!(
                        cmd.contains(env_prefix),
                        "'{input}' must preserve env prefix '{env_prefix}', got '{cmd}'"
                    );
                }
                other => panic!("Expected Rewrite for '{input}', got {other:?}"),
            }
        }
    }

    #[test]
    fn test_env_prefix_multi_var_routes() {
        let input = "NODE_ENV=test CI=1 npx vitest run";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk vitest"),
                    "must route to rtk vitest, got '{cmd}'"
                );
                assert!(
                    cmd.contains("NODE_ENV=test"),
                    "must preserve NODE_ENV, got '{cmd}'"
                );
                assert!(cmd.contains("CI=1"), "must preserve CI, got '{cmd}'");
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_env_prefix_unknown_cmd_fallback() {
        assert_passthrough("VAR=1 unknown_xyz_abc_cmd");
    }

    #[test]
    fn test_env_prefix_npm_still_passthrough() {
        assert_passthrough("NODE_ENV=test npm run test:e2e");
    }

    #[test]
    fn test_env_prefix_docker_compose_passthrough() {
        assert_passthrough("COMPOSE_PROJECT_NAME=test docker compose up -d");
    }

    #[test]
    fn test_global_options_not_blocked() {
        let cases = [
            "git --no-pager status",
            "git -C /path/to/project status",
            "git -C /path --no-pager log --oneline",
            "git --no-optional-locks diff HEAD",
            "git --bare log",
            "cargo +nightly test",
            "cargo +stable build --release",
            "docker --context prod ps",
            "docker -H tcp://host:2375 images",
            "kubectl -n kube-system get pods",
            "kubectl --context prod describe pod foo",
        ];
        for input in cases {
            assert_passthrough(input);
        }
    }

    #[test]
    fn test_specific_commands_not_blocked() {
        let cases = [
            "git log --oneline -10",
            "git diff HEAD",
            "git show abc123",
            "git add .",
            "gh pr list",
            "gh api repos/owner/repo",
            "gh release list",
            "npm run test:e2e",
            "npm run build",
            "npm test",
            "docker compose up -d",
            "docker compose logs postgrest",
            "docker compose down",
            "docker run --rm postgres",
            "docker exec -it db psql",
            "kubectl describe pod foo",
            "kubectl apply -f deploy.yaml",
            "npx playwright test",
            "npx prisma migrate",
            "cargo test",
            "vitest",
            "vitest run",
            "vitest run --reporter=verbose",
            "npx vitest run",
            "pnpm vitest run --coverage",
            "vue-tsc -b",
            "npx vue-tsc --noEmit",
            "curl -s https://example.com",
            "ls -la",
            "grep -rn pattern src/",
            "rg pattern src/",
        ];
        for input in cases {
            assert!(
                matches!(check_for_hook(input, "claude"), HookResult::Rewrite(_)),
                "'{}' should Rewrite (not Blocked)",
                input
            );
        }
    }

    #[test]
    fn test_builtins_not_blocked() {
        let cases = [
            "echo hello world",
            "cd /tmp",
            "mkdir -p foo/bar",
            "python3 script.py",
            "find . -name '*.ts'",
            "tree src/",
            "wget https://example.com/file",
        ];
        for input in cases {
            assert_passthrough(input);
        }
        assert_passthrough("node -e 'console.log(1)'");
    }

    #[test]
    fn test_noglob_prefix_routes_inner_command() {
        assert_rewrite("noglob gh pr view 123", "noglob rtk gh pr view 123");
    }

    #[test]
    fn test_noglob_prefix_with_unknown_command() {
        match check_for_hook("noglob some-unknown-tool --arg", "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c 'noglob"),
                    "noglob should not be inside rtk run -c, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_command_prefix_routes_inner_command() {
        assert_rewrite("command git status", "command rtk git status");
    }

    #[test]
    fn test_builtin_prefix_passthrough() {
        match check_for_hook("builtin cd /tmp", "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c 'builtin"),
                    "builtin should not be inside rtk run -c, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_nocorrect_prefix_routes_inner_command() {
        assert_rewrite("nocorrect git log -10", "nocorrect rtk git log");
    }

    #[test]
    fn test_noglob_gh_release_create_exact_bug_report() {
        let input = "noglob gh release create v0.3.0-rc1 --title v0.3.0-rc1 --notes test --prerelease --draft";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    !cmd.contains("rtk run -c 'noglob"),
                    "noglob must not be inside rtk run -c, got '{}'",
                    cmd
                );
                assert!(
                    cmd.starts_with("noglob "),
                    "noglob must be the outermost prefix, got '{}'",
                    cmd
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_nested_shell_prefixes() {
        assert_rewrite("noglob command git status", "noglob command rtk git status");
    }

    #[test]
    fn test_shell_prefix_plus_env_prefix() {
        assert_rewrite(
            "noglob GIT_PAGER=cat git log -10",
            "noglob GIT_PAGER=cat rtk git log",
        );
    }

    #[test]
    fn test_exec_prefix_routes_inner_command() {
        assert_rewrite("exec git status", "exec rtk git status");
    }

    #[test]
    fn test_bare_shell_prefix_passthrough() {
        match check_for_hook("noglob", "claude") {
            HookResult::Rewrite(cmd) => {
                assert_eq!(cmd, "noglob", "bare prefix should pass through unchanged");
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_unknown_command_passthrough() {
        assert_passthrough("gh release create v0.3.0 --title test");
    }

    #[test]
    fn test_full_path_binary_routes_correctly() {
        assert_rewrite("/opt/homebrew/bin/git status", "rtk git status");
    }

    #[test]
    fn test_full_path_unknown_command_passthrough() {
        assert_passthrough("/opt/homebrew/bin/gh release create v0.3.0");
    }

    #[test]
    fn test_env_prefix_unknown_command_passthrough() {
        assert_passthrough("GH_DEBUG= gh release create v0.3.0");
    }

    #[test]
    fn test_noglob_unknown_command_passthrough() {
        assert_passthrough("noglob gh release create v0.3.0");
    }

    #[test]
    fn test_chain_mixed_known_unknown() {
        match check_for_hook("gh release create v1 && git status", "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run -c"), "chains still need rtk run -c");
                assert!(cmd.contains("rtk git status"), "known cmd routed");
                assert!(
                    cmd.contains("gh release create v1"),
                    "unknown cmd preserved"
                );
            }
            HookResult::Blocked(_) => panic!("should not be blocked"),
        }
    }

    #[test]
    fn test_gh_release_create_exact_bug_report() {
        let input = r#"gh release create v0.3.0 --title "ai_session_tools v0.3.0" --notes-file notes/v0.3.0-release.md"#;
        assert_passthrough(input);
    }

    #[test]
    fn test_completely_unknown_binary_passthrough() {
        assert_passthrough("some-custom-tool --flag value");
    }

    #[test]
    fn test_compound_commands_rewrite() {
        let cases = [
            ("cd /tmp && git status", "&&"),
            ("cd dir && git status && git diff", "&&"),
            ("git add . && git commit -m msg", "&&"),
            ("echo start ; git status ; echo done", ";"),
            ("git pull || echo failed", "||"),
        ];
        for (input, operator) in cases {
            match check_for_hook(input, "claude") {
                HookResult::Rewrite(cmd) => {
                    assert!(cmd.contains("rtk run"), "'{input}' should rewrite");
                    assert!(
                        cmd.contains(operator),
                        "'{input}' must preserve '{operator}', got '{cmd}'"
                    );
                }
                other => panic!("Expected Rewrite for '{input}', got {other:?}"),
            }
        }
    }

    #[test]
    fn test_compound_quoted_operators_not_split() {
        let input = r#"git commit -m "Fix && Bug""#;
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk git commit"),
                    "Quoted && must not split; should route to rtk git commit, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite for quoted &&, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_2_redirect_routes_to_rtk() {
        let input = "cargo test 2>&1";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk cargo"),
                    "must use rtk cargo filter, got '{cmd}'"
                );
                assert!(
                    cmd.contains("2>&1"),
                    "must preserve 2>&1 suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_dev_null_routes_to_rtk() {
        let input = "cargo test 2>/dev/null";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk cargo"), "must use rtk cargo, got '{cmd}'");
                assert!(
                    cmd.contains("/dev/null"),
                    "must preserve /dev/null suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_pipe_tee_routes_to_rtk() {
        let input = "cargo test | tee /tmp/log.txt";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk cargo"),
                    "must use rtk cargo filter, got '{cmd}'"
                );
                assert!(cmd.contains("tee"), "must preserve tee suffix, got '{cmd}'");
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_pipe_head_routes_to_rtk() {
        let input = "git log | head -20";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk git"), "must use rtk git, got '{cmd}'");
                assert!(
                    cmd.contains("head"),
                    "must preserve head suffix, got '{cmd}'"
                );
                assert!(
                    !cmd.contains("rtk run -c"),
                    "must NOT fall back to passthrough, got '{cmd}'"
                );
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_suffix_unknown_cmd_still_passthrough() {
        assert_passthrough("unknown_xyz_cmd 2>&1");
    }

    #[test]
    fn test_suffix_unsafe_pipe_still_passthrough() {
        let input = "cargo test | grep FAILED";
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                let _ = cmd;
            }
            other => panic!("Expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_token_waste_allowed_in_pipelines() {
        let cases = [
            "cat file.txt | grep pattern",
            "cat file.txt > output.txt",
            "sed 's/old/new/' file.txt > output.txt",
            "head -n 10 file.txt | grep pattern",
            "for f in *.txt; do cat \"$f\" | grep x; done",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    #[test]
    fn test_different_agents_same_result() {
        for agent in ["claude", "gemini"] {
            match check_for_hook("git status", agent) {
                HookResult::Rewrite(_) => {}
                other => panic!("Expected Rewrite for agent '{}', got {:?}", agent, other),
            }
        }
    }

    #[test]
    fn test_format_for_claude() {
        let (output, success, code) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".to_string()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert_eq!(code, 0);

        let (output, success, code) =
            format_for_claude(HookResult::Blocked("Error message".to_string()));
        assert_eq!(output, "Error message");
        assert!(!success);
        assert_eq!(code, 2);
    }

    #[test]
    fn test_dollar_var_routes_natively() {
        let result = match check_for_hook("git log $BRANCH", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk git"),
            "Expected rtk git routing for 'git log $BRANCH', got: {}",
            result
        );
        assert!(
            !result.contains("rtk run"),
            "Should not fall to passthrough for simple $VAR, got: {}",
            result
        );
    }

    #[test]
    fn test_dollar_subshell_still_passthrough() {
        let result = match check_for_hook("git log $(git rev-parse HEAD)", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk run"),
            "Subshell $(...) must route to passthrough, got: {}",
            result
        );
    }

    #[test]
    fn test_rewrite_depth_limit_blocked() {
        assert_blocked("echo hello", MAX_REWRITE_DEPTH, "loop");
    }

    #[test]
    fn test_rewrite_depth_limit_allowed() {
        match check_for_hook_inner("echo hello", 0) {
            HookResult::Rewrite(cmd) => assert_eq!(cmd, "echo hello"),
            _ => panic!("Expected Rewrite at depth 0"),
        }
    }

    #[test]
    fn test_claude_rewrite_exit_code_is_zero() {
        let (_, _, code) = format_for_claude(HookResult::Rewrite("rtk run -c 'ls'".into()));
        assert_eq!(code, 0, "Rewrite must exit 0 (success)");
    }

    #[test]
    fn test_claude_block_exit_code_is_two() {
        let (_, _, code) = format_for_claude(HookResult::Blocked("denied".into()));
        assert_eq!(
            code, 2,
            "Block must exit 2 (blocking error per Claude Code spec)"
        );
    }

    #[test]
    fn test_claude_rewrite_output_is_command_text() {
        let (output, success, _) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".into()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert!(
            !output.starts_with('{'),
            "Rewrite output must be plain text, not JSON"
        );
    }

    #[test]
    fn test_claude_block_output_is_human_message() {
        let (output, success, _) =
            format_for_claude(HookResult::Blocked("Use Read tool instead".into()));
        assert_eq!(output, "Use Read tool instead");
        assert!(!success);
        assert!(
            !output.starts_with('{'),
            "Block output must be plain text, not JSON"
        );
    }

    #[test]
    fn test_claude_rewrite_success_flag_true() {
        let (_, success, _) = format_for_claude(HookResult::Rewrite("cmd".into()));
        assert!(success, "Rewrite must set success=true");
    }

    #[test]
    fn test_claude_block_success_flag_false() {
        let (_, success, _) = format_for_claude(HookResult::Blocked("msg".into()));
        assert!(!success, "Block must set success=false");
    }

    #[test]
    fn test_claude_exit_codes_not_one() {
        let (_, _, rewrite_code) = format_for_claude(HookResult::Rewrite("cmd".into()));
        let (_, _, block_code) = format_for_claude(HookResult::Blocked("msg".into()));
        assert_ne!(
            rewrite_code, 1,
            "Exit code 1 is non-blocking error, not valid for rewrite"
        );
        assert_ne!(
            block_code, 1,
            "Exit code 1 is non-blocking error, not valid for block"
        );
    }

    #[test]
    fn test_cross_protocol_safe_command_allowed_by_both() {
        for cmd in ["git status", "cargo test", "ls -la", "echo hello"] {
            let claude = check_for_hook(cmd, "claude");
            let gemini = check_for_hook(cmd, "gemini");
            match (&claude, &gemini) {
                (HookResult::Rewrite(_), HookResult::Rewrite(_)) => {}
                _ => panic!(
                    "'{}': Claude={:?}, Gemini={:?} -- both should Rewrite",
                    cmd, claude, gemini
                ),
            }
        }
    }

    #[test]
    fn test_routing_native_commands() {
        let cases = [
            ("git status", "rtk git status"),
            ("git log --oneline -10", "rtk git log --oneline -10"),
            ("git diff HEAD", "rtk git diff HEAD"),
            ("git add .", "rtk git add ."),
            ("git commit -m msg", "rtk git commit"),
            ("gh pr view 156", "rtk gh pr view 156"),
            ("cargo test", "rtk cargo test"),
            (
                "cargo clippy --all-targets",
                "rtk cargo clippy --all-targets",
            ),
            ("grep -r pattern src/", "rtk grep -r pattern src/"),
            ("rg pattern src/", "rtk grep pattern src/"),
            ("ls -la", "rtk ls -la"),
            ("vitest", "rtk vitest run"),
            ("vitest run", "rtk vitest run"),
            ("vitest run --coverage", "rtk vitest run --coverage"),
            ("pnpm test", "rtk vitest run"),
            ("pnpm vitest", "rtk vitest run"),
            ("pnpm lint", "rtk lint"),
            ("pnpm eslint src/", "rtk lint"),
            ("pnpm eslint .", "rtk lint ."),
            ("pnpm eslint --fix src/", "rtk lint"),
            ("npx tsc --noEmit", "rtk tsc --noEmit"),
            ("python -m pytest tests/", "rtk pytest tests/"),
            ("uv pip list", "rtk pip list"),
            ("go test ./...", "rtk go test ./..."),
            ("go build ./...", "rtk go build ./..."),
            ("go vet ./...", "rtk go vet ./..."),
            ("eslint src/", "rtk lint src/"),
            ("tsc --noEmit", "rtk tsc --noEmit"),
            ("prettier src/", "rtk prettier src/"),
            ("playwright test", "rtk playwright test"),
            ("prisma migrate dev", "rtk prisma migrate dev"),
            (
                "curl https://api.example.com",
                "rtk curl https://api.example.com",
            ),
            ("pytest tests/", "rtk pytest tests/"),
            ("pytest -x tests/unit", "rtk pytest -x tests/unit"),
            ("golangci-lint run ./...", "rtk golangci-lint run ./..."),
            ("docker ps", "rtk docker ps"),
            ("docker images", "rtk docker images"),
            ("docker logs mycontainer", "rtk docker logs mycontainer"),
            ("kubectl get pods", "rtk kubectl get pods"),
            ("kubectl logs mypod", "rtk kubectl logs mypod"),
            ("ruff check src/", "rtk ruff check src/"),
            ("ruff format src/", "rtk ruff format src/"),
            ("pip list", "rtk pip list"),
            ("pip install requests", "rtk pip install requests"),
            ("pip outdated", "rtk pip outdated"),
            ("pip show requests", "rtk pip show requests"),
            ("gh issue list", "rtk gh issue list"),
            ("gh run view 123", "rtk gh run view 123"),
            ("git stash pop", "rtk git stash pop"),
            ("git fetch origin", "rtk git fetch origin"),
            ("gt log", "rtk gt log"),
            ("gt submit", "rtk gt submit"),
            ("gt sync", "rtk gt sync"),
            ("gt create feat/new-branch", "rtk gt create feat/new-branch"),
        ];
        for (input, expected) in cases {
            assert_rewrite(input, expected);
        }
    }

    #[test]
    fn test_routing_subcommand_filter_fallback() {
        let cases = [
            "docker build .",
            "docker run -it nginx",
            "kubectl apply -f dep.yaml",
            "kubectl delete pod mypod",
            "go mod tidy",
            "go generate ./...",
            "ruff lint src/",
            "pip freeze",
            "pip uninstall requests",
            "cargo publish",
            "cargo run",
            "git rebase -i HEAD~3",
            "git cherry-pick abc123",
            "gh repo clone foo/bar",
        ];
        for input in cases {
            assert_passthrough(input);
        }
    }

    #[test]
    fn test_routing_vitest_no_double_run() {
        // ISSUE #112: shell script sed bug produces "rtk vitest run run --coverage"
        let result = match check_for_hook("pnpm vitest run --coverage", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert_rewrite("pnpm vitest run --coverage", "rtk vitest run --coverage");
        assert!(
            !result.contains("run run"),
            "Must not double 'run' in output: '{}'",
            result
        );
    }

    #[test]
    fn test_routing_fallbacks_to_rtk_run() {
        let chain_cases = ["git add . && git commit -m msg", "git log | grep fix"];
        for input in chain_cases {
            assert_rewrite(input, "rtk run -c");
        }
        let passthrough_cases = [
            "git checkout main",
            "tail -n 20 file.txt",
            "tail -f server.log",
        ];
        for input in passthrough_cases {
            assert_passthrough(input);
        }
    }

    #[test]
    fn test_cross_agent_routing_identical() {
        for cmd in ["git status", "cargo test", "ls -la"] {
            let claude_result = check_for_hook(cmd, "claude");
            let gemini_result = check_for_hook(cmd, "gemini");
            match (&claude_result, &gemini_result) {
                (HookResult::Rewrite(c), HookResult::Rewrite(g)) => {
                    assert_eq!(c, g, "claude and gemini must route '{}' identically", cmd);
                    assert!(
                        !c.contains("rtk run -c"),
                        "'{}' should not fall back to rtk run -c",
                        cmd
                    );
                }
                _ => panic!(
                    "'{}' should Rewrite for both agents: claude={:?} gemini={:?}",
                    cmd, claude_result, gemini_result
                ),
            }
        }
    }

    #[test]
    fn test_chain_both_commands_substituted() {
        let result = match check_for_hook("cargo test && git log", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be substituted to rtk cargo inside chain: {}",
            result
        );
        assert!(
            result.contains("rtk git"),
            "git log must be substituted to rtk git inside chain: {}",
            result
        );
        assert!(
            result.contains("rtk run"),
            "chain still needs shell wrapper (rtk run -c): {}",
            result
        );
    }

    #[test]
    fn test_chain_with_dollar_var_substituted() {
        let result = match check_for_hook("cargo test && git log $BRANCH", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be rtk in chain: {}",
            result
        );
        assert!(
            result.contains("rtk git log"),
            "git log $BRANCH must be rtk with var preserved: {}",
            result
        );
        assert!(
            result.contains("$BRANCH"),
            "$BRANCH must be preserved in rewritten chain: {}",
            result
        );
    }

    #[test]
    fn test_chain_unknown_command_not_substituted() {
        let result = match check_for_hook("cargo test && unknown_xyz_cmd", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo test must be substituted to rtk: {}",
            result
        );
        assert!(
            result.contains("unknown_xyz_cmd"),
            "unknown command must pass through unchanged: {}",
            result
        );
        assert!(
            !result.contains("rtk unknown"),
            "must not invent rtk subcommands for unknown binary: {}",
            result
        );
    }

    #[test]
    fn test_semicolon_chain_substituted() {
        let result = match check_for_hook("cargo test ; git status", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo must be rtk in semicolon chain: {}",
            result
        );
        assert!(
            result.contains("rtk git"),
            "git must be rtk in semicolon chain: {}",
            result
        );
    }

    #[test]
    fn test_or_chain_substituted() {
        let result = match check_for_hook("cargo test || go test ./...", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert!(
            result.contains("rtk cargo"),
            "cargo must be rtk in || chain: {}",
            result
        );
        assert!(
            result.contains("rtk go"),
            "go must be rtk in || chain: {}",
            result
        );
    }

    #[test]
    fn test_format_preserving_contains_expected() {
        assert!(
            FORMAT_PRESERVING.contains(&"tail"),
            "tail is format-preserving (line-per-line passthrough)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"echo"),
            "echo is format-preserving (output equals input)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"find"),
            "find is format-preserving (path-per-line)"
        );
        assert!(
            FORMAT_PRESERVING.contains(&"cat"),
            "cat is format-preserving (byte passthrough)"
        );
    }

    #[test]
    fn test_format_changing_not_in_format_preserving() {
        assert!(
            !FORMAT_PRESERVING.contains(&"cargo"),
            "cargo test compresses output -- not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"git"),
            "git log/diff compresses output -- not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"pytest"),
            "pytest compresses output -- not format-preserving"
        );
        assert!(
            !FORMAT_PRESERVING.contains(&"go"),
            "go test compresses output -- not format-preserving"
        );
    }

    #[test]
    fn test_transparent_sinks_contains_expected() {
        assert!(
            TRANSPARENT_SINKS.contains(&"tee"),
            "tee is a transparent sink (copies stdin to file + stdout)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"head"),
            "head is a transparent sink (truncates lines)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"cat"),
            "cat is a transparent sink (passes through)"
        );
        assert!(
            TRANSPARENT_SINKS.contains(&"tail"),
            "tail is a transparent sink (last N lines)"
        );
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn exec(cmd: &str) -> String {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        let out = std::process::Command::new(parts[0])
            .args(&parts[1..])
            .output()
            .unwrap_or_else(|e| panic!("failed to exec '{cmd}': {e}"));
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and git repo"]
    fn test_e2e_git_status_saves_tokens() {
        let raw_cmd = "git status";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk git"),
            "lexer+router should produce rtk git status, got: {rtk_cmd}"
        );

        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(raw_tok > 0, "raw git status produced no output");

        let savings = 100.0 * (1.0 - rtk_tok as f64 / raw_tok as f64);
        assert!(
            savings >= 40.0,
            "rtk git status should save >=40% tokens vs raw git status, \
             got {savings:.1}% ({raw_tok} raw -> {rtk_tok} rtk tokens)"
        );
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and directory with files"]
    fn test_e2e_ls_saves_tokens() {
        let raw_cmd = "ls -la .";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk ls"),
            "lexer+router should produce rtk ls, got: {rtk_cmd}"
        );

        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(raw_tok > 0, "raw ls -la produced no output");

        let savings = 100.0 * (1.0 - rtk_tok as f64 / raw_tok as f64);
        assert!(
            savings >= 40.0,
            "rtk ls should save >=40% tokens vs raw ls -la, \
             got {savings:.1}% ({raw_tok} raw -> {rtk_tok} rtk tokens)"
        );
    }

    #[test]
    #[ignore = "requires installed rtk binary (cargo install --path .) and git repo with history"]
    fn test_e2e_git_log_saves_tokens() {
        let raw_cmd = "git log --oneline -20";
        let rtk_cmd = match check_for_hook(raw_cmd, "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite for '{raw_cmd}', got {other:?}"),
        };
        assert!(
            rtk_cmd.starts_with("rtk git"),
            "lexer+router should produce rtk git log, got: {rtk_cmd}"
        );

        let raw_out = exec(raw_cmd);
        let rtk_out = exec(&rtk_cmd);
        let raw_tok = count_tokens(&raw_out);
        let rtk_tok = count_tokens(&rtk_out);
        assert!(
            raw_tok > 0,
            "raw git log produced no output -- need a repo with commits"
        );

        let ratio = rtk_tok as f64 / raw_tok.max(1) as f64;
        assert!(
            ratio <= 1.05,
            "rtk git log must not significantly bloat output vs raw git log \
             ({raw_tok} raw -> {rtk_tok} rtk, ratio {ratio:.2})"
        );
    }

    #[test]
    fn test_cat_multi_file_rewrites_to_rtk_read() {
        let result = check_for_hook("cat file1.txt file2.txt", "claude");
        assert!(
            matches!(&result, HookResult::Rewrite(s) if s == "rtk read file1.txt file2.txt"),
            "cat (multi-file) must rewrite to rtk read on this branch; got: {:?}",
            result
        );
    }

    #[test]
    fn test_cat_single_file_rewrites_to_rtk_read() {
        let result = check_for_hook("cat CLAUDE.md", "claude");
        assert!(
            matches!(&result, HookResult::Rewrite(s) if s == "rtk read CLAUDE.md"),
            "cat (single-file) must rewrite to rtk read on this branch; got: {:?}",
            result
        );
    }

    // ISSUE #196: gh --json/--jq/--template passthrough
    #[test]
    fn test_gh_json_flag_passes_through() {
        assert!(should_passthrough("gh pr list --json number,title"));
        assert!(should_passthrough(
            "gh pr list --json number --jq '.[].number'"
        ));
        assert!(should_passthrough("gh pr view 42 --template '{{.title}}'"));
        assert!(should_passthrough("gh api repos/owner/repo --jq '.name'"));
    }

    #[test]
    fn test_gh_without_json_not_passthrough() {
        assert!(!should_passthrough("gh pr list"));
        assert!(!should_passthrough("gh issue list"));
    }

    #[test]
    fn test_hook_lookup_git_branch() {
        assert_eq!(hook_lookup("git", "branch"), Some(("rtk git", "git")));
    }

    #[test]
    fn test_hook_lookup_git_worktree() {
        assert_eq!(hook_lookup("git", "worktree"), Some(("rtk git", "git")));
    }

    #[test]
    fn test_git_branch_routes_via_hook() {
        assert_rewrite("git branch", "rtk git branch");
    }

    #[test]
    fn test_git_worktree_list_routes_via_hook() {
        assert_rewrite("git worktree list", "rtk git worktree");
    }
}
