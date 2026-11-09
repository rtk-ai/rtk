//! Compacts native dotnet tool list tables without dropping tool or manifest data.

pub fn filter_output(raw: &str) -> String {
    compact_table(raw).unwrap_or_else(|| raw.to_string())
}

fn compact_table(raw: &str) -> Option<String> {
    let mut lines = raw.lines();
    let header = lines.next()?;
    let mut columns = vec![
        (0, "Package Id"),
        (header.find("Version")?, "Version"),
        (header.find("Commands")?, "Commands"),
    ];
    if let Some(start) = header.find("Manifest") {
        columns.push((start, "Manifest"));
    }

    for (index, &(start, name)) in columns.iter().enumerate() {
        let end = columns.get(index + 1).map_or(header.len(), |c| c.0);
        if header.get(start..end)?.trim() != name {
            return None;
        }
    }
    let separator = lines.next()?.trim();
    if separator.len() < header.trim_end().len() || !separator.bytes().all(|b| b == b'-') {
        return None;
    }

    let mut output = columns.iter().map(|c| c.1).collect::<Vec<_>>().join("\t");
    output.push('\n');
    let last_column_start = columns.last()?.0;
    let mut rows = 0;
    for line in lines.filter(|line| !line.trim().is_empty()) {
        // SDK 8 and 10 pad by Unicode text elements, not UTF-8 bytes. Only use
        // header byte offsets when the fixed columns are ASCII; keep the rest raw.
        if !line.get(..last_column_start)?.is_ascii() {
            return None;
        }
        let mut cells = Vec::with_capacity(columns.len());
        for (index, &(start, _)) in columns.iter().enumerate() {
            // Header offsets preserve spaces inside a command list or manifest path.
            // A missing column boundary means this is not the SDK table we recognize.
            if start > 0 && line.as_bytes().get(start - 1) != Some(&b' ') {
                return None;
            }
            let end = columns.get(index + 1).map_or(line.len(), |c| c.0);
            let cell = line.get(start..end)?.trim();
            if cell.is_empty() {
                return None;
            }
            cells.push(cell);
        }
        if !cells[0]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            || !cells[1].bytes().next()?.is_ascii_digit()
            || cells[1].contains(char::is_whitespace)
        {
            return None;
        }
        output.push_str(&cells.join("\t"));
        output.push('\n');
        rows += 1;
    }
    (rows > 0).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{guard::never_worse, tracking::estimate_tokens};

    const GLOBAL: &str = include_str!("../../../tests/fixtures/dotnet/tool_list_global.txt");
    const LOCAL: &str = include_str!("../../../tests/fixtures/dotnet/tool_list_local.txt");

    #[test]
    fn keeps_all_tools_and_reduces_real_sdk_output() {
        for raw in [GLOBAL, LOCAL] {
            let filtered = filter_output(raw);
            let shown = never_worse(raw, &filtered);
            assert!(shown.contains("csharp-ls\t0.28.0\tcsharp-ls"));
            assert!(shown.contains("dotnet-outdated-tool\t4.8.1\tdotnet-outdated"));
            let savings = 1.0 - estimate_tokens(shown) as f64 / estimate_tokens(raw) as f64;
            assert!(
                savings >= 0.2,
                "expected >=20% reduction, got {:.1}%",
                savings * 100.0
            );
        }
    }

    #[test]
    fn keeps_multiple_commands_and_unicode_manifest_paths() {
        let raw = format!(
            "{:<24}{:<20}{:<32}Manifest\r\n{}\r\n{:<24}{:<20}{:<32}/workspace/工具  sample/.config/dotnet-tools.json\r\n",
            "Package Id",
            "Version",
            "Commands",
            "-".repeat(84),
            "sample.tool",
            "1.2.3-beta.1",
            "first-command, second-command"
        );
        assert_eq!(
            filter_output(&raw),
            "Package Id\tVersion\tCommands\tManifest\nsample.tool\t1.2.3-beta.1\tfirst-command, second-command\t/workspace/工具  sample/.config/dotnet-tools.json\n"
        );
    }

    #[test]
    fn keeps_sdk8_global_tables_with_multiple_commands() {
        let raw = include_str!("../../../tests/fixtures/dotnet/tool_list_sdk8_global.txt");
        assert_eq!(
            filter_output(raw),
            "Package Id\tVersion\tCommands\nsample.tool\t1.2.3\tfirst-command, second-command\nother.tool\t2.0.0-beta.1\tother-command\n"
        );
    }

    #[test]
    fn keeps_sdk8_local_tables_with_windows_manifest_paths() {
        let raw = include_str!("../../../tests/fixtures/dotnet/tool_list_sdk8_local.txt");
        assert_eq!(
            filter_output(raw),
            "Package Id\tVersion\tCommands\tManifest\nsample.tool\t1.2.3\tfirst-command, second-command\tC:\\workspace\\Tool Suite\\.config\\dotnet-tools.json\nother.tool\t2.0.0-beta.1\tother-command\tC:\\workspace\\Tool Suite\\.config\\dotnet-tools.json\n"
        );
    }

    #[test]
    fn leaves_json_output_unchanged() {
        for raw in [
            include_str!("../../../tests/fixtures/dotnet/tool_list_json_global.txt"),
            include_str!("../../../tests/fixtures/dotnet/tool_list_json_local.txt"),
            include_str!("../../../tests/fixtures/dotnet/tool_list_json_empty.txt"),
        ] {
            assert_eq!(filter_output(raw), raw);
        }
    }

    #[test]
    fn unicode_commands_cannot_shift_into_the_manifest_column() {
        let raw = include_str!("../../../tests/fixtures/dotnet/tool_list_unicode_commands.txt");
        assert_eq!(filter_output(raw), raw);
    }

    #[test]
    fn unknown_or_incomplete_output_is_preserved_whole() {
        for raw in [
            "".to_string(),
            include_str!("../../../tests/fixtures/dotnet/tool_list_empty.txt").to_string(),
            "Package Id  Commands  Version\n------------------------------\n".to_string(),
            "Package Id  Version  Commands  New Column\n-----------------------------------------\n".to_string(),
            format!("{GLOBAL}Warning: additional SDK information\n"),
            GLOBAL.replace("csharp-ls                 0.28.0       csharp-ls", "truncated"),
            GLOBAL.replace("---", "   "),
        ] {
            assert_eq!(filter_output(&raw), raw);
        }
    }

    #[test]
    fn every_row_is_preserved_without_a_list_cap() {
        let row = GLOBAL.lines().nth(2).expect("fixture has a tool row");
        let header = GLOBAL.lines().take(2).collect::<Vec<_>>().join("\n");
        let raw = format!("{header}\n{}", format!("{row}\n").repeat(150));
        let filtered = filter_output(&raw);
        assert_eq!(filtered.lines().count(), 151);
        assert_eq!(
            filtered.matches("csharp-ls\t0.28.0\tcsharp-ls").count(),
            150
        );
    }
}
