use crate::learn::detector::CorrectionRule;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// Built-in sanitization patterns, ordered by specificity.
// AWS resource IDs must run before account IDs to avoid partial hex matches.
lazy_static! {
    static ref AWS_RESOURCE_ID_RE: Regex = Regex::new(
        r"\b(vpc|sg|subnet|vpce|igw|rtb|acl|eni|vol|snap|nat|eipalloc|pcx)-[0-9a-f]{8,17}\b"
    ).unwrap();

    static ref AWS_INSTANCE_ID_RE: Regex = Regex::new(
        r"\bi-[0-9a-f]{8,17}\b"
    ).unwrap();

    static ref ROUTE53_ZONE_ID_RE: Regex = Regex::new(
        r"\bZ[0-9A-Z]{10,32}\b"
    ).unwrap();

    // Matches 12-digit account IDs only after `::`, `:`, or `/` delimiters.
    // Avoids false positives on bare numbers (timestamps, ports, file sizes).
    static ref AWS_ACCOUNT_ID_RE: Regex = Regex::new(
        r"(::?|/)\d{12}\b"
    ).unwrap();

    static ref USER_HOME_PATH_RE: Regex = Regex::new(
        r"/(Users|home)/[a-zA-Z0-9._-]+/"
    ).unwrap();

    static ref GITHUB_ORG_REPO_RE: Regex = Regex::new(
        r"(github\.com/|repos/)([a-zA-Z0-9._-]+)/([a-zA-Z0-9._-]+)"
    ).unwrap();

    // UUIDs (Databricks account IDs, API keys, correlation IDs, etc.)
    static ref UUID_RE: Regex = Regex::new(
        r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b"
    ).unwrap();

    // --repo org/repo flag (not a URL, so GITHUB_ORG_REPO_RE won't catch it)
    static ref REPO_FLAG_RE: Regex = Regex::new(
        r"--repo\s+([a-zA-Z0-9._-]+)/([a-zA-Z0-9._-]+)"
    ).unwrap();
}

/// Encapsulates sanitization config: whether enabled + user-defined patterns from config.toml.
/// Constructed once per `rtk learn` invocation, passed by reference to all output functions.
pub struct Sanitizer {
    enabled: bool,
    user_patterns: Vec<Regex>,
}

impl Sanitizer {
    /// Create a sanitizer. When enabled, loads user patterns from config.toml `[learn].sanitize_patterns`.
    pub fn new(enabled: bool) -> Self {
        let user_patterns = if enabled {
            load_user_patterns()
        } else {
            Vec::new()
        };
        Self {
            enabled,
            user_patterns,
        }
    }

    /// Sanitize a string by applying built-in and user patterns.
    /// Returns borrowed input unchanged when disabled or when no patterns match.
    pub fn sanitize<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if !self.enabled || input.is_empty() {
            return Cow::Borrowed(input);
        }

        // Track whether any pattern matched. Only allocate when something changes.
        let mut owned: Option<String> = None;

        macro_rules! apply {
            ($re:expr, $replacement:expr) => {
                let current = owned.as_deref().unwrap_or(input);
                let result = $re.replace_all(current, $replacement);
                if let Cow::Owned(new) = result {
                    owned = Some(new);
                }
            };
        }

        apply!(AWS_RESOURCE_ID_RE, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            format!("{}-<ID>", prefix)
        });
        apply!(AWS_INSTANCE_ID_RE, "i-<ID>");
        apply!(ROUTE53_ZONE_ID_RE, "Z<ZONE_ID>");
        apply!(AWS_ACCOUNT_ID_RE, |caps: &regex::Captures| {
            let delim = caps.get(1).map_or("", |m| m.as_str());
            format!("{}<ACCOUNT_ID>", delim)
        });
        apply!(USER_HOME_PATH_RE, "~/");
        apply!(GITHUB_ORG_REPO_RE, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            format!("{}<org>/<repo>", prefix)
        });
        apply!(UUID_RE, "<UUID>");
        apply!(REPO_FLAG_RE, "--repo <org>/<repo>");

        for re in &self.user_patterns {
            apply!(re, "<REDACTED>");
        }

        match owned {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(input),
        }
    }
}

/// Load user-defined sanitization patterns from config.
/// Returns compiled regexes, logging warnings for invalid patterns.
fn load_user_patterns() -> Vec<Regex> {
    let config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    config
        .learn
        .sanitize_patterns
        .iter()
        .filter_map(|p| match Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                eprintln!(
                    "[rtk learn] Warning: invalid sanitize pattern '{}': {}",
                    p, e
                );
                None
            }
        })
        .collect()
}

pub fn format_console_report(
    rules: &[CorrectionRule],
    total_corrections: usize,
    sessions: usize,
    days: u64,
    sanitizer: &Sanitizer,
) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "RTK Learn -- {} rules from {} corrections ({} sessions, {} days)\n",
        rules.len(),
        total_corrections,
        sessions,
        days
    ));

    if rules.is_empty() {
        output.push_str("\nNo CLI corrections detected.\n");
        return output;
    }

    output.push('\n');

    for rule in rules {
        let count_marker = if rule.occurrences > 1 {
            format!("[{}x] ", rule.occurrences)
        } else {
            "     ".to_string()
        };

        let wrong = sanitizer.sanitize(&rule.wrong_pattern);
        let right = sanitizer.sanitize(&rule.right_pattern);

        output.push_str(&format!("{}{}  →  {}\n", count_marker, wrong, right));

        let error_line = rule.example_error.lines().next().unwrap_or("").trim();
        if !error_line.is_empty() {
            let error_display = sanitizer.sanitize(error_line);
            output.push_str(&format!("     Error: {}\n", error_display));
        }
    }

    output
}

pub fn write_rules_file(rules: &[CorrectionRule], path: &str, sanitizer: &Sanitizer) -> Result<()> {
    let path_obj = Path::new(path);

    if let Some(parent) = path_obj.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = String::new();
    content.push_str("# CLI Corrections (auto-generated by rtk learn)\n");
    content.push_str("# Run `rtk learn --write-rules` to update\n\n");

    if rules.is_empty() {
        content.push_str("No CLI corrections detected yet.\n");
        fs::write(path, content)?;
        return Ok(());
    }

    let mut grouped: HashMap<String, Vec<&CorrectionRule>> = HashMap::new();
    for rule in rules {
        grouped
            .entry(rule.base_command.clone())
            .or_default()
            .push(rule);
    }

    let mut base_commands: Vec<String> = grouped.keys().cloned().collect();
    base_commands.sort();

    for base_cmd in base_commands {
        let rules_for_cmd = grouped.get(&base_cmd).unwrap();

        let section_header = capitalize_first(&base_cmd);
        content.push_str(&format!("## {}\n", section_header));

        for rule in rules_for_cmd {
            let occurrence_note = if rule.occurrences > 1 {
                format!(" (seen {}x)", rule.occurrences)
            } else {
                String::new()
            };

            let right = sanitizer.sanitize(&rule.right_pattern);
            let wrong = sanitizer.sanitize(&rule.wrong_pattern);

            content.push_str(&format!(
                "- Use `{}` not `{}`{}\n",
                right, wrong, occurrence_note
            ));
        }

        content.push('\n');
    }

    fs::write(path, content)?;
    Ok(())
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learn::detector::ErrorType;

    fn enabled_sanitizer() -> Sanitizer {
        Sanitizer {
            enabled: true,
            user_patterns: Vec::new(),
        }
    }

    fn disabled_sanitizer() -> Sanitizer {
        Sanitizer {
            enabled: false,
            user_patterns: Vec::new(),
        }
    }

    // --- Built-in pattern: AWS resource IDs ---

    #[test]
    fn sanitize_redacts_aws_resource_ids() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("--vpc-ids vpc-0abc123def456789a").as_ref(),
            "--vpc-ids vpc-<ID>"
        );
        assert_eq!(
            s.sanitize("--group-id sg-038bad75fae68765e").as_ref(),
            "--group-id sg-<ID>"
        );
        assert_eq!(
            s.sanitize("vpce-0e5a4b26aac99e9bd subnet-0abc123def456789a")
                .as_ref(),
            "vpce-<ID> subnet-<ID>"
        );
    }

    #[test]
    fn sanitize_redacts_ec2_instance_ids() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("--instance-ids i-0abc123def456789a").as_ref(),
            "--instance-ids i-<ID>"
        );
    }

    // --- Built-in pattern: Route53 ---

    #[test]
    fn sanitize_redacts_route53_zone_ids() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("--hosted-zone-id Z0247406X6CI60Z1JQ2").as_ref(),
            "--hosted-zone-id Z<ZONE_ID>"
        );
    }

    // --- Built-in pattern: AWS account IDs (context-sensitive) ---

    #[test]
    fn sanitize_redacts_account_ids_after_delimiters() {
        let s = enabled_sanitizer();
        // After `::` in ARNs
        assert_eq!(
            s.sanitize("arn:aws:iam::123456789012:role/MyRole").as_ref(),
            "arn:aws:iam::<ACCOUNT_ID>:role/MyRole"
        );
        // After `/` in paths
        assert_eq!(
            s.sanitize("accounts/123456789012/settings").as_ref(),
            "accounts/<ACCOUNT_ID>/settings"
        );
    }

    #[test]
    fn sanitize_ignores_bare_12_digit_numbers() {
        let s = enabled_sanitizer();
        assert_eq!(s.sanitize("--port 8443").as_ref(), "--port 8443");
        assert_eq!(
            s.sanitize("some_value 123456789012").as_ref(),
            "some_value 123456789012"
        );
    }

    // --- Built-in pattern: user home paths ---

    #[test]
    fn sanitize_redacts_user_home_paths() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("/Users/johndoe/projects/myapp/src/main.rs")
                .as_ref(),
            "~/projects/myapp/src/main.rs"
        );
        assert_eq!(
            s.sanitize("/home/deploy/services/api").as_ref(),
            "~/services/api"
        );
    }

    // --- Built-in pattern: GitHub org/repo ---

    #[test]
    fn sanitize_redacts_github_org_repo_in_urls() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("gh api repos/my-company/my-service/pulls")
                .as_ref(),
            "gh api repos/<org>/<repo>/pulls"
        );
        assert_eq!(
            s.sanitize("git clone https://github.com/acme-corp/platform.git")
                .as_ref(),
            "git clone https://github.com/<org>/<repo>"
        );
    }

    // --- Built-in pattern: UUIDs ---

    #[test]
    fn sanitize_redacts_uuids() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("--account-id 292bf5a3-e432-483f-b14d-949b412ea11a")
                .as_ref(),
            "--account-id <UUID>"
        );
        assert_eq!(
            s.sanitize("correlation_id=a1b2c3d4-e5f6-7890-abcd-ef1234567890")
                .as_ref(),
            "correlation_id=<UUID>"
        );
    }

    #[test]
    fn sanitize_redacts_uuids_in_quotes() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize(r#"export ID="292bf5a3-e432-483f-b14d-949b412ea11a""#)
                .as_ref(),
            r#"export ID="<UUID>""#
        );
    }

    #[test]
    fn sanitize_ignores_short_hex_dashes() {
        let s = enabled_sanitizer();
        // Short segments that look like UUID fragments but aren't full UUIDs
        assert_eq!(s.sanitize("tag-abc-123").as_ref(), "tag-abc-123");
    }

    // --- Built-in pattern: --repo flag ---

    #[test]
    fn sanitize_redacts_repo_flag() {
        let s = enabled_sanitizer();
        assert_eq!(
            s.sanitize("gh pr diff 123 --repo my-company/services")
                .as_ref(),
            "gh pr diff 123 --repo <org>/<repo>"
        );
        assert_eq!(
            s.sanitize("rtk proxy gh pr diff 5721 --repo acme-corp/platform 2>/dev/null")
                .as_ref(),
            "rtk proxy gh pr diff 5721 --repo <org>/<repo> 2>/dev/null"
        );
    }

    // --- Safe content / edge cases ---

    #[test]
    fn sanitize_preserves_commands_without_sensitive_data() {
        let s = enabled_sanitizer();
        let safe = "git commit --amend -m 'fix typo'";
        let result = s.sanitize(safe);
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "no-match should be zero-copy"
        );
        assert_eq!(result.as_ref(), safe);
    }

    #[test]
    fn sanitize_handles_empty_input() {
        let s = enabled_sanitizer();
        assert_eq!(s.sanitize("").as_ref(), "");
    }

    #[test]
    fn sanitize_applies_multiple_patterns_in_one_command() {
        let s = enabled_sanitizer();
        let input = "aws ec2 describe-security-groups --profile prod --group-ids sg-038bad75fae68765e --query 'SecurityGroups[0]' --output yaml 2>&1";
        let result = s.sanitize(input);
        assert!(result.contains("sg-<ID>"), "sg ID not redacted");
        assert!(!result.contains("038bad75fae68765e"), "raw hex leaked");
    }

    // --- User-defined patterns ---

    #[test]
    fn sanitize_applies_user_patterns_from_config() {
        let s = Sanitizer {
            enabled: true,
            user_patterns: vec![
                Regex::new(r"acme-corp\.1password\.com").unwrap(),
                Regex::new(r"internal\.example\.co").unwrap(),
            ],
        };
        assert_eq!(
            s.sanitize("op read 'op://Vault/Item' --account acme-corp.1password.com")
                .as_ref(),
            "op read 'op://Vault/Item' --account <REDACTED>"
        );
        assert_eq!(
            s.sanitize("kubectl get nodes -l internal.example.co/id")
                .as_ref(),
            "kubectl get nodes -l <REDACTED>/id"
        );
    }

    #[test]
    fn sanitize_user_patterns_combine_with_builtins() {
        let s = Sanitizer {
            enabled: true,
            user_patterns: vec![Regex::new(r"secret-project").unwrap()],
        };
        let input = "aws ec2 describe-vpcs --vpc-ids vpc-0abc123def456789a --tag secret-project";
        let result = s.sanitize(input);
        assert!(result.contains("vpc-<ID>"), "built-in didn't fire");
        assert!(result.contains("<REDACTED>"), "user pattern didn't fire");
        assert!(!result.contains("secret-project"), "user data leaked");
    }

    #[test]
    fn sanitize_multiple_user_patterns_chain_correctly() {
        let s = Sanitizer {
            enabled: true,
            user_patterns: vec![
                Regex::new(r"first-secret").unwrap(),
                Regex::new(r"second-secret").unwrap(),
            ],
        };
        let input = "cmd --a first-secret --b second-secret";
        let result = s.sanitize(input);
        assert!(!result.contains("first-secret"), "first pattern missed");
        assert!(!result.contains("second-secret"), "second pattern missed");
        assert_eq!(result.as_ref(), "cmd --a <REDACTED> --b <REDACTED>");
    }

    // --- Disabled sanitizer ---

    #[test]
    fn disabled_sanitizer_returns_input_unchanged() {
        let s = disabled_sanitizer();
        let input = "aws ec2 describe-vpcs --vpc-ids vpc-0abc123def456789a";
        // Cow::Borrowed means zero allocation
        let result = s.sanitize(input);
        assert!(matches!(result, Cow::Borrowed(_)), "should be zero-copy");
        assert_eq!(result.as_ref(), input);
    }

    // --- Console report ---

    #[test]
    fn format_console_report_shows_header_for_empty_rules() {
        let s = disabled_sanitizer();
        let report = format_console_report(&[], 0, 0, 30, &s);
        assert!(report.contains("0 rules"));
        assert!(report.contains("No CLI corrections detected"));
    }

    #[test]
    fn format_console_report_includes_counts_and_errors() {
        let s = disabled_sanitizer();
        let rules = vec![
            CorrectionRule {
                wrong_pattern: "git commit --ammend".to_string(),
                right_pattern: "git commit --amend".to_string(),
                error_type: ErrorType::UnknownFlag,
                occurrences: 3,
                base_command: "git commit".to_string(),
                example_error: "error: unexpected argument '--ammend'".to_string(),
            },
            CorrectionRule {
                wrong_pattern: "gh pr edit -t".to_string(),
                right_pattern: "gh pr edit --title".to_string(),
                error_type: ErrorType::UnknownFlag,
                occurrences: 1,
                base_command: "gh pr".to_string(),
                example_error: "unknown flag: -t".to_string(),
            },
        ];

        let report = format_console_report(&rules, 4, 10, 30, &s);
        assert!(report.contains("2 rules"));
        assert!(report.contains("4 corrections"));
        assert!(report.contains("[3x]"));
        assert!(report.contains("--ammend"));
        assert!(report.contains("Error: error: unexpected argument"));
    }

    #[test]
    fn format_console_report_redacts_when_sanitized() {
        let s = enabled_sanitizer();
        let rules = vec![CorrectionRule {
            wrong_pattern: "aws ec2 describe-vpcs --vpc-ids vpc-0abc123def456789a".to_string(),
            right_pattern: "aws ec2 describe-vpcs --output table".to_string(),
            error_type: ErrorType::Other("General Error".to_string()),
            occurrences: 1,
            base_command: "aws ec2".to_string(),
            example_error: "error: invalid vpc id".to_string(),
        }];

        let report = format_console_report(&rules, 1, 1, 30, &s);
        assert!(report.contains("vpc-<ID>"));
        assert!(!report.contains("0abc123def456789a"));
    }

    // --- Markdown rules file ---

    #[test]
    fn write_rules_file_produces_grouped_markdown() {
        let s = disabled_sanitizer();
        let rules = vec![CorrectionRule {
            wrong_pattern: "git commit --ammend".to_string(),
            right_pattern: "git commit --amend".to_string(),
            error_type: ErrorType::UnknownFlag,
            occurrences: 3,
            base_command: "git commit".to_string(),
            example_error: "error: unexpected argument '--ammend'".to_string(),
        }];

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("cli-corrections.md");
        let path_str = path.to_str().unwrap();

        write_rules_file(&rules, path_str, &s).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# CLI Corrections"));
        assert!(content.contains("## Git commit"));
        assert!(content.contains("Use `git commit --amend` not `git commit --ammend`"));
        assert!(content.contains("(seen 3x)"));
    }

    #[test]
    fn write_rules_file_redacts_when_sanitized() {
        let s = enabled_sanitizer();
        let rules = vec![CorrectionRule {
            wrong_pattern: "aws ec2 describe-security-groups --group-ids sg-038bad75fae68765e"
                .to_string(),
            right_pattern: "aws ec2 describe-security-groups --output table".to_string(),
            error_type: ErrorType::Other("General Error".to_string()),
            occurrences: 1,
            base_command: "aws ec2".to_string(),
            example_error: "error".to_string(),
        }];

        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("cli-corrections.md");
        let path_str = path.to_str().unwrap();

        write_rules_file(&rules, path_str, &s).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("sg-<ID>"));
        assert!(!content.contains("038bad75fae68765e"));
    }
}
