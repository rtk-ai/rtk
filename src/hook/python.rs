use super::helpers::replace_prefix;

pub fn try_rewrite_python(match_cmd: &str, cmd_body: &str) -> Option<String> {
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
}
