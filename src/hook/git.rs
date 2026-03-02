use super::helpers::replace_prefix;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Strips git global flags before subcommand: -C path, -c key=val, --no-pager, etc.
    static ref GIT_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(-C|-c)\s+\S+\s*|--[a-z-]+=\S+\s*|--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*)").unwrap();
}

pub fn try_rewrite_git(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_git = match_cmd.strip_prefix("git ").unwrap_or("");

    // P1-1: -C /path changes working directory — stripping it silently
    // would run the command in the wrong directory with no warning.
    if after_git.split_whitespace().any(|t| t == "-C") {
        return None;
    }

    let stripped = GIT_GLOBAL_FLAGS_RE.replace_all(after_git, "");
    let stripped = stripped.trim_start();

    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "status" | "diff" | "log" | "add" | "commit" | "push" | "pull" | "branch" | "fetch"
        | "stash" | "show" | "worktree" => {
            // Strip git global flags from the rewritten command too,
            // since RTK doesn't understand --no-pager, -C, etc.
            let after_body_git = cmd_body.strip_prefix("git ").unwrap_or(cmd_body);
            let clean_body = GIT_GLOBAL_FLAGS_RE.replace_all(after_body_git, "");
            let clean_body = clean_body.trim_start();
            Some(format!("rtk git {}", clean_body))
        }
        _ => None,
    }
}

pub fn try_rewrite_gh(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // Skip rewrite when gh outputs structured data (JSON/jq/template) —
    // scripts and Claude Code parse this directly, RTK would corrupt it.
    if match_cmd.contains(" --json")
        || match_cmd.contains(" --jq")
        || match_cmd.contains(" --template")
    {
        return None;
    }

    let after_gh = match_cmd.strip_prefix("gh ").unwrap_or("");
    let subcmd = after_gh.split_whitespace().next().unwrap_or("");
    match subcmd {
        "pr" | "issue" | "run" | "api" | "release" => {
            Some(replace_prefix(cmd_body, "gh ", "rtk gh "))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::rewrite_chain;

    fn rewrite(cmd: &str) -> Option<String> {
        if cmd.contains("<<") {
            return None;
        }
        rewrite_chain(cmd)
    }

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
        // Global flags are stripped — RTK doesn't understand them
        assert_eq!(rewrite("git --no-pager diff"), Some("rtk git diff".into()));
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

    // --- P0-1: gh --json/--jq/--template should NOT be rewritten ---
    #[test]
    fn test_gh_pr_list_json_no_rewrite() {
        assert_eq!(rewrite("gh pr list --json number"), None);
    }

    #[test]
    fn test_gh_pr_view_json_no_rewrite() {
        assert_eq!(rewrite("gh pr view 123 --json title,body"), None);
    }

    #[test]
    fn test_gh_issue_list_jq_no_rewrite() {
        assert_eq!(rewrite("gh issue list --jq '.[].title'"), None);
    }

    #[test]
    fn test_gh_pr_list_template_no_rewrite() {
        assert_eq!(rewrite("gh pr list --template '{{.title}}'"), None);
    }

    #[test]
    fn test_gh_api_json_no_rewrite() {
        assert_eq!(rewrite("gh api repos/foo/bar --jq '.name'"), None);
    }

    // --- P0-2: git global flags should be stripped in rewritten output ---
    #[test]
    fn test_git_no_pager_log_strips_flag() {
        assert_eq!(
            rewrite("git --no-pager log --oneline"),
            Some("rtk git log --oneline".into())
        );
    }

    #[test]
    fn test_git_no_pager_diff_strips_flag() {
        assert_eq!(rewrite("git --no-pager diff"), Some("rtk git diff".into()));
    }

    #[test]
    fn test_git_c_path_no_rewrite() {
        // P1-1: -C changes working directory — can't strip safely
        assert_eq!(rewrite("git -C /some/path status"), None);
    }

    #[test]
    fn test_git_no_optional_locks_strips_flag() {
        assert_eq!(
            rewrite("git --no-optional-locks status"),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_git_bare_strips_flag() {
        assert_eq!(
            rewrite("git --bare log --oneline -5"),
            Some("rtk git log --oneline -5".into())
        );
    }

    #[test]
    fn test_git_c_config_strips_flag() {
        assert_eq!(
            rewrite("git -c core.pager=cat log"),
            Some("rtk git log".into())
        );
    }

    #[test]
    fn test_git_unknown_global_flag_no_rewrite() {
        // Unknown boolean flags cause the subcommand to not be detected → safe fallback (no rewrite)
        assert_eq!(rewrite("git --worktree-attributes log --oneline"), None);
    }

    // --- P1-1: git -C /path changes working directory — must NOT be rewritten ---
    #[test]
    fn test_git_dash_c_path_no_rewrite() {
        // -C changes working directory — stripping it would run in wrong directory
        assert_eq!(rewrite("git -C /other/repo status"), None);
    }

    #[test]
    fn test_git_dash_c_path_log_no_rewrite() {
        assert_eq!(rewrite("git -C /other/repo log --oneline"), None);
    }

    #[test]
    fn test_git_dash_c_path_diff_no_rewrite() {
        assert_eq!(rewrite("git -C ../sibling diff HEAD"), None);
    }

    #[test]
    fn test_git_lowercase_c_config_still_strips() {
        // -c key=val is a config override, safe to strip (no directory change)
        assert_eq!(
            rewrite("git -c core.pager=cat log"),
            Some("rtk git log".into())
        );
    }
}
