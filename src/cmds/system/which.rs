use anyhow::Result;

use crate::core::utils::resolve_binary;

pub fn run(name: &str) -> Result<i32> {
    if name.contains('/') || name.contains('\\') {
        eprintln!("{}", path_like_name_error(name));
        return Ok(1);
    }

    match resolve_binary(name) {
        Ok(path) => {
            println!("{}", path.display());
            Ok(0)
        }
        Err(_) => {
            eprintln!("rtk which: {name} not found");
            Ok(1)
        }
    }
}

fn path_like_name_error(name: &str) -> String {
    format!("rtk which: path-like name '{name}' is unsupported; pass a command name from PATH")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_missing_returns_1() {
        let code = run("rtk-definitely-missing-command-for-test").unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn which_rejects_path_like_name() {
        let code = run("foo/bar").unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn path_like_name_error_is_actionable() {
        assert_eq!(
            path_like_name_error("foo/bar"),
            "rtk which: path-like name 'foo/bar' is unsupported; pass a command name from PATH"
        );
    }
}
