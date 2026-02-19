/// Replace a prefix in cmd_body. Simple string replacement of first occurrence.
pub fn replace_prefix(cmd_body: &str, old_prefix: &str, new_prefix: &str) -> String {
    if let Some(rest) = cmd_body.strip_prefix(old_prefix) {
        format!("{}{}", new_prefix, rest)
    } else {
        // Fallback: just prepend rtk
        format!("rtk {}", cmd_body)
    }
}

/// Check if s starts with any of the given prefixes (exact or followed by space)
pub fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|p| s == *p || s.starts_with(&format!("{} ", p)))
}
