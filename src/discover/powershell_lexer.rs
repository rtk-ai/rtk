pub fn parse_static_argv(raw: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut saw_token = false;

    while let Some(ch) = chars.next() {
        if in_single {
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    current.push('\'');
                } else {
                    in_single = false;
                }
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double {
            if ch == '"' {
                in_double = false;
            } else if ch == '$' || ch == '`' {
                return None;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            c if c.is_whitespace() => {
                if saw_token {
                    args.push(std::mem::take(&mut current));
                    saw_token = false;
                }
            }
            '\'' => {
                in_single = true;
                saw_token = true;
            }
            '"' => {
                in_double = true;
                saw_token = true;
            }
            '$' if is_unc_share_dollar(&current, chars.peek().copied()) => {
                current.push(ch);
                saw_token = true;
            }
            '$' | '`' | '{' | '}' | ';' => return None,
            _ => {
                current.push(ch);
                saw_token = true;
            }
        }
    }

    if in_single || in_double {
        return None;
    }
    if saw_token {
        args.push(current);
    }
    if args.iter().any(|arg| arg == "--%") {
        return None;
    }
    Some(args)
}

fn is_unc_share_dollar(token_prefix: &str, next: Option<char>) -> bool {
    if let Some(ch) = next {
        if ch != '\\' && !ch.is_whitespace() {
            return false;
        }
    }

    let Some(unc_path) = token_prefix.strip_prefix(r"\\") else {
        return false;
    };
    let mut components = unc_path.split('\\');
    let server = components.next().unwrap_or_default();
    let share = components.next().unwrap_or_default();

    !server.is_empty() && !share.is_empty() && components.next().is_none()
}

pub fn render_static_argv(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty()
                || arg.chars().any(|c| c.is_whitespace())
                || arg.contains('\'')
                || arg.contains('"')
            {
                format!("'{}'", arg.replace('\'', "''"))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_quoted_literal_with_doubled_quote() {
        assert_eq!(
            parse_static_argv("Get-Content 'can''t.txt'"),
            Some(vec!["Get-Content".to_string(), "can't.txt".to_string()])
        );
    }

    #[test]
    fn rejects_interpolated_double_quoted_value() {
        assert_eq!(parse_static_argv(r#"Get-Content "$env:TEMP\a.txt""#), None);
    }

    #[test]
    fn rejects_stop_parsing_token() {
        assert_eq!(parse_static_argv("Get-Content --% literal.txt"), None);
    }
}
