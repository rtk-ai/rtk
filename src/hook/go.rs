use super::helpers::replace_prefix;

pub fn try_rewrite_go(match_cmd: &str, cmd_body: &str) -> Option<String> {
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
}
