//! Cross-platform path utilities for Windows and Unix.

/// Normalize a path to use forward slashes and handle Windows paths correctly.
pub fn normalize_path(path: &str) -> String {
    // First, replace backslashes with forward slashes for cross-platform handling
    let path = path.replace('\\', "/");

    // Handle Windows drive letters (e.g., "C:/Users/..." -> "C:/Users/...")
    // This is already handled by the replace above

    path
}

/// Extract the basename from a path (handles both / and \ separators).
pub fn path_basename(path: &str) -> &str {
    // Scan the caller's string for the last separator rather than building a
    // normalized copy: the copy is a local, so returning a slice of it does not
    // borrow-check, and the allocation bought nothing.
    match path.rfind(['/', '\\']) {
        Some(sep) => &path[sep + 1..],
        None => path,
    }
}

/// Strip absolute path prefix from a command, keeping only the binary name.
/// Handles both Unix (/usr/bin/grep) and Windows (C:\Windows\System32\cmd.exe) paths.
pub fn strip_absolute_path(cmd: &str) -> String {
    let (program, rest) = split_program(cmd);
    if !program.contains('/') && !program.contains('\\') {
        return cmd.to_string();
    }
    let basename = path_basename(program);
    if basename.is_empty() {
        return cmd.to_string();
    }
    format!("{basename}{rest}")
}

/// Split a command into its program and the remainder, which keeps its leading
/// whitespace so the two can simply be concatenated back together.
fn split_program(cmd: &str) -> (&str, &str) {
    // A quoted program is unambiguous: `"C:\Program Files\app.exe" --flag`.
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(close) = rest.find('"') {
            return (&rest[..close], &rest[close + 1..]);
        }
    }
    match executable_token_end(cmd) {
        Some(end) => cmd.split_at(end),
        None => match cmd.find(' ') {
            Some(pos) => cmd.split_at(pos),
            None => (cmd, ""),
        },
    }
}

/// Byte offset just past a leading unquoted Windows program path that contains
/// spaces, e.g. `C:\Program Files\Git\bin\git.exe`.
///
/// Splitting on the first space is right on Unix but cuts `C:\Program Files\…`
/// in half. This only engages when the first token already looks like a path,
/// so `ls /tmp/a.exe b` is not mistaken for a program named `ls /tmp/a.exe`.
fn executable_token_end(cmd: &str) -> Option<usize> {
    const EXTS: [&str; 5] = [".exe", ".bat", ".cmd", ".com", ".ps1"];
    let first = cmd.split(' ').next()?;
    if !first.contains('/') && !first.contains('\\') {
        return None;
    }

    let ends_with_exe = |s: &str| {
        let lower = s.to_ascii_lowercase();
        EXTS.iter().any(|ext| lower.ends_with(ext))
    };

    let mut from = 0;
    while let Some(rel) = cmd[from..].find(' ') {
        let end = from + rel;
        if ends_with_exe(&cmd[..end]) {
            return Some(end);
        }
        from = end + 1;
    }
    ends_with_exe(cmd).then_some(cmd.len())
}

/// Compact a path for display, keeping only the most relevant parts.
/// Handles both Unix and Windows paths.
pub fn compact_path(path: &str, max_len: usize) -> String {
    let path = normalize_path(path);

    if path.len() <= max_len {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    // Try to keep the first part and last two parts
    let compacted = format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    );

    if compacted.len() <= max_len {
        compacted
    } else {
        // Fallback: just show the last part
        format!(".../{}", parts.last().unwrap_or(&""))
    }
}

/// Compact a file path for error/formatter output.
/// Looks for common project directories (src, lib, tests) and strips everything before them.
pub fn compact_file_path(path: &str) -> String {
    let path = normalize_path(path);

    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/tests/") {
        format!("tests/{}", &path[pos + 7..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_unix() {
        assert_eq!(normalize_path("/usr/bin/grep"), "/usr/bin/grep");
    }

    #[test]
    fn test_normalize_path_windows() {
        assert_eq!(
            normalize_path(r"C:\Windows\System32\cmd.exe"),
            "C:/Windows/System32/cmd.exe"
        );
        assert_eq!(
            normalize_path(r"C:\Users\foo\project\src\main.rs"),
            "C:/Users/foo/project/src/main.rs"
        );
    }

    #[test]
    fn test_path_basename() {
        assert_eq!(path_basename("/usr/bin/grep"), "grep");
        assert_eq!(path_basename(r"C:\Windows\System32\cmd.exe"), "cmd.exe");
        assert_eq!(path_basename("relative/path/file.txt"), "file.txt");
    }

    #[test]
    fn test_strip_absolute_path_unix() {
        let result = strip_absolute_path("/usr/bin/grep -rn foo");
        assert_eq!(result, "grep -rn foo");

        let result = strip_absolute_path("/usr/bin/git status");
        assert_eq!(result, "git status");
    }

    #[test]
    fn test_strip_absolute_path_windows() {
        let result = strip_absolute_path(r"C:\Windows\System32\cmd.exe /c dir");
        assert_eq!(result, "cmd.exe /c dir");

        let result = strip_absolute_path(r"C:\Program Files\Git\bin\git.exe status");
        assert_eq!(result, "git.exe status");
    }

    #[test]
    fn test_strip_absolute_path_no_path() {
        let result = strip_absolute_path("grep -rn foo");
        assert_eq!(result, "grep -rn foo");

        let result = strip_absolute_path("git status");
        assert_eq!(result, "git status");
    }

    #[test]
    fn test_compact_path() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path, 50);
        assert!(compact.len() <= 50);
        assert!(compact.contains("..."));

        let path = r"C:\Users\foo\project\src\components\Button.tsx";
        let compact = compact_path(path, 50);
        assert!(compact.len() <= 50);
    }

    #[test]
    fn test_compact_file_path() {
        assert_eq!(
            compact_file_path("/home/user/project/src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(
            compact_file_path(r"C:\Users\foo\project\lib\utils.py"),
            "lib/utils.py"
        );
        assert_eq!(
            compact_file_path("/home/user/project/tests/test.py"),
            "tests/test.py"
        );
        assert_eq!(compact_file_path("relative/file.py"), "file.py");
    }
}
