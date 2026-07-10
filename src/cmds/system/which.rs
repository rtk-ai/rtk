use anyhow::Result;

use crate::core::utils::resolve_binary;

pub fn run(name: &str) -> Result<i32> {
    if name.contains('/') || name.contains('\\') {
        eprintln!("rtk which: {name} not found");
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
}
