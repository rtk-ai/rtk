use std::process::Command;

pub(crate) fn has_unstaged_changes() -> bool {
    Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false)
}

pub(crate) fn is_interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

pub(crate) fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/".to_string());
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    }
}

pub(crate) fn get_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_expand_tilde_simple() {
        let home = env::var("HOME").unwrap_or("/".to_string());
        assert_eq!(expand_tilde("~/src"), format!("{}/src", home));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        let home = env::var("HOME").unwrap_or("/".to_string());
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn test_expand_tilde_relative() {
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    #[test]
    fn test_get_home_returns_something() {
        let home = get_home();
        assert!(!home.is_empty());
    }

    #[test]
    fn test_is_interactive_returns_false_in_tests() {
        assert!(!is_interactive());
    }

    #[test]
    fn test_has_unstaged_changes_does_not_panic() {
        let _result: bool = has_unstaged_changes();
    }
}
