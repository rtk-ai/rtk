use super::helpers::replace_prefix;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Strips git global flags before subcommand: -C path, -c key=val, --no-pager, etc.
    static ref GIT_GLOBAL_FLAGS_RE: Regex =
        Regex::new(r"(?:(-C|-c)\s+\S+\s*|--[a-z-]+=\S+\s*|--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*)").unwrap();
}

pub fn try_rewrite_git(_match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_git = _match_cmd.strip_prefix("git ").unwrap_or("");
    let stripped = GIT_GLOBAL_FLAGS_RE.replace_all(after_git, "");
    let stripped = stripped.trim_start();

    let subcmd = stripped.split_whitespace().next().unwrap_or("");
    match subcmd {
        "status" | "diff" | "log" | "add" | "commit" | "push" | "pull" | "branch" | "fetch"
        | "stash" | "show" | "worktree" => Some(format!("rtk {}", cmd_body)),
        _ => None,
    }
}

pub fn try_rewrite_gh(match_cmd: &str, cmd_body: &str) -> Option<String> {
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
}
