use std::fs::OpenOptions;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;

pub fn run(path: &Path) -> Result<i32> {
    if path.is_dir() {
        eprintln!("rtk touch: {} is a directory", path.display());
        return Ok(1);
    }

    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => {
            if let Err(err) = file.set_modified(SystemTime::now()) {
                eprintln!("rtk touch: {}: {}", path.display(), err);
                return Ok(1);
            }
            Ok(0)
        }
        Err(err) => {
            eprintln!("rtk touch: {}: {}", path.display(), err);
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn touch_creates_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("created.txt");
        assert_eq!(run(&path).unwrap(), 0);
        assert!(path.is_file());
    }

    #[test]
    fn touch_preserves_existing_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("existing.txt");
        fs::write(&path, "hello").unwrap();
        assert_eq!(run(&path).unwrap(), 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn touch_updates_mtime() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mtime.txt");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .unwrap();
        writeln!(file, "content").unwrap();
        let old = UNIX_EPOCH + Duration::from_secs(1);
        file.set_modified(old).unwrap();
        drop(file);

        assert_eq!(run(&path).unwrap(), 0);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(modified > old);
    }

    #[test]
    fn touch_rejects_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(run(temp.path()).unwrap(), 1);
    }
}
