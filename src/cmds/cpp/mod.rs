automod::dir!(pub "src/cmds/cpp");

pub(crate) fn failure_fallback(tool: &str, exit_code: i32, raw: &str) -> String {
    const MAX_LINES: usize = 39;
    const MAX_CHARS: usize = 4096;

    let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    let mut excerpt = Vec::new();
    if lines.len() <= MAX_LINES {
        excerpt.extend(lines.iter().copied());
    } else {
        excerpt.extend(lines.iter().take(28).copied());
        excerpt.push("... [output omitted] ...");
        excerpt.extend(lines.iter().skip(lines.len() - 10).copied());
    }

    let mut out = format!("{}: failed (exit {})", tool, exit_code);
    let mut truncated = false;
    let mut emitted_omission_marker = false;
    for line in excerpt {
        let room = MAX_CHARS.saturating_sub(out.chars().count() + 1);
        if room < 3 {
            truncated = true;
            break;
        }
        let line = crate::core::utils::truncate(line, room.min(512));
        if out.chars().count() + line.chars().count() + 1 > MAX_CHARS {
            truncated = true;
            break;
        }
        out.push('\n');
        out.push_str(&line);
        emitted_omission_marker |= line == "... [output omitted] ...";
    }
    if truncated && !emitted_omission_marker {
        let marker = "\n... [output omitted] ...";
        let keep = MAX_CHARS.saturating_sub(marker.chars().count());
        if out.chars().count() > keep {
            out = out.chars().take(keep).collect();
        }
        out.push_str(marker);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::failure_fallback;

    #[test]
    fn failure_fallback_line_cap_is_bounded_and_marked() {
        let raw = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = failure_fallback("cmake", 1, &raw);
        assert!(out.lines().count() <= 40);
        assert!(out.contains("output omitted"));
    }

    #[test]
    fn failure_fallback_character_cap_is_bounded_and_marked() {
        let raw = (0..100)
            .map(|i| format!("line {} {}", i, "x".repeat(500)))
            .collect::<Vec<_>>()
            .join("\n");
        let out = failure_fallback("cmake", 1, &raw);
        assert!(out.chars().count() <= 4096);
        assert!(out.contains("output omitted"));
    }
}
