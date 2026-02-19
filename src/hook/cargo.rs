use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Cargo toolchain prefix: cargo +nightly ...
    static ref CARGO_TOOLCHAIN_RE: Regex =
        Regex::new(r"^\+\S+\s+").unwrap();
}

pub fn try_rewrite_cargo(match_cmd: &str, cmd_body: &str) -> Option<String> {
    let after_cargo = match_cmd.strip_prefix("cargo ").unwrap_or("");
    let after_toolchain = CARGO_TOOLCHAIN_RE.replace(after_cargo, "");
    let subcmd = after_toolchain.split_whitespace().next().unwrap_or("");
    match subcmd {
        "test" | "build" | "clippy" | "check" | "install" | "fmt" => {
            Some(format!("rtk {}", cmd_body))
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
}
