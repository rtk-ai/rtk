use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, PartialEq, Eq)]
struct MkdirSpec {
    path: PathBuf,
}

pub fn run(args: &[String]) -> Result<i32> {
    let spec = match parse(args) {
        Ok(spec) => spec,
        Err(message) => {
            eprintln!("rtk mkdir: {message}");
            return Ok(2);
        }
    };

    if spec.path.exists() && !spec.path.is_dir() {
        eprintln!("rtk mkdir: {} exists and is not a directory", spec.path.display());
        return Ok(1);
    }

    if let Err(err) = std::fs::create_dir_all(&spec.path) {
        eprintln!("rtk mkdir: {}: {}", spec.path.display(), err);
        return Ok(1);
    }

    if !spec.path.is_dir() {
        eprintln!("rtk mkdir: {} is not a directory", spec.path.display());
        return Ok(1);
    }

    Ok(0)
}

fn parse(args: &[String]) -> std::result::Result<MkdirSpec, String> {
    let mut parents = false;
    let mut path: Option<PathBuf> = None;

    for token in args {
        if token == "-p" || token == "--parents" {
            if parents {
                return Err("duplicate -p/--parents".to_string());
            }
            parents = true;
            continue;
        }
        if token.starts_with('-') {
            return Err(format!("unsupported option: {token}"));
        }
        if path.is_some() {
            return Err("multiple paths are unsupported".to_string());
        }
        path = Some(PathBuf::from(token));
    }

    if !parents {
        return Err("requires -p or --parents".to_string());
    }

    Ok(MkdirSpec {
        path: path.ok_or_else(|| "missing path".to_string())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn mkdir_p_creates_nested_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("a").join("b").join("c");
        let args = vec!["-p".to_string(), path.to_string_lossy().to_string()];
        assert_eq!(run(&args).unwrap(), 0);
        assert!(path.is_dir());
    }

    #[test]
    fn mkdir_p_existing_directory_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("existing");
        fs::create_dir(&path).unwrap();
        let args = vec![path.to_string_lossy().to_string(), "-p".to_string()];
        assert_eq!(run(&args).unwrap(), 0);
    }

    #[test]
    fn mkdir_without_p_rejected() {
        let args = vec!["target".to_string()];
        assert_eq!(run(&args).unwrap(), 2);
    }

    #[test]
    fn mkdir_p_existing_file_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.txt");
        fs::write(&path, "content").unwrap();
        let args = vec!["-p".to_string(), path.to_string_lossy().to_string()];
        assert_eq!(run(&args).unwrap(), 1);
    }
}
