use super::helpers::{replace_prefix, starts_with_any};

pub fn try_rewrite_js_ts(match_cmd: &str, cmd_body: &str) -> Option<String> {
    // vitest (with optional pnpm/npx prefix)
    if starts_with_any(match_cmd, &["vitest", "pnpm vitest", "npx vitest"]) {
        let rest = match_cmd
            .trim_start_matches("pnpm ")
            .trim_start_matches("npx ")
            .trim_start_matches("vitest")
            .trim_start_matches(" run")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk vitest run".to_string()
        } else {
            format!("rtk vitest run {}", rest)
        });
    }

    // pnpm test → rtk vitest run
    if match_cmd == "pnpm test" || match_cmd.starts_with("pnpm test ") {
        let rest = match_cmd.strip_prefix("pnpm test").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk vitest run".to_string()
        } else {
            format!("rtk vitest run {}", rest)
        });
    }

    // npm test
    if match_cmd == "npm test" || match_cmd.starts_with("npm test ") {
        return Some(replace_prefix(cmd_body, "npm test", "rtk npm test"));
    }

    // npm run
    if match_cmd.starts_with("npm run ") {
        return Some(replace_prefix(cmd_body, "npm run ", "rtk npm "));
    }

    // vue-tsc (with optional npx prefix)
    if match_cmd == "vue-tsc"
        || match_cmd.starts_with("vue-tsc ")
        || match_cmd == "npx vue-tsc"
        || match_cmd.starts_with("npx vue-tsc ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("vue-tsc")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // pnpm tsc
    if match_cmd == "pnpm tsc" || match_cmd.starts_with("pnpm tsc ") {
        let rest = match_cmd.strip_prefix("pnpm tsc").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // tsc (with optional npx prefix)
    if match_cmd == "tsc"
        || match_cmd.starts_with("tsc ")
        || match_cmd == "npx tsc"
        || match_cmd.starts_with("npx tsc ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("tsc")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk tsc".to_string()
        } else {
            format!("rtk tsc {}", rest)
        });
    }

    // pnpm lint
    if match_cmd == "pnpm lint" || match_cmd.starts_with("pnpm lint ") {
        let rest = match_cmd.strip_prefix("pnpm lint").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk lint".to_string()
        } else {
            format!("rtk lint {}", rest)
        });
    }

    // pnpm eslint (direct binary invocation)
    if match_cmd == "pnpm eslint" || match_cmd.starts_with("pnpm eslint ") {
        let rest = match_cmd.strip_prefix("pnpm eslint").unwrap_or("").trim();
        return Some(if rest.is_empty() {
            "rtk lint".to_string()
        } else {
            format!("rtk lint {}", rest)
        });
    }

    // eslint (with optional npx prefix)
    if match_cmd == "eslint"
        || match_cmd.starts_with("eslint ")
        || match_cmd == "npx eslint"
        || match_cmd.starts_with("npx eslint ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("eslint")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk lint".to_string()
        } else {
            format!("rtk lint {}", rest)
        });
    }

    // prettier (with optional npx prefix)
    if match_cmd == "prettier"
        || match_cmd.starts_with("prettier ")
        || match_cmd == "npx prettier"
        || match_cmd.starts_with("npx prettier ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("prettier")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk prettier".to_string()
        } else {
            format!("rtk prettier {}", rest)
        });
    }

    // playwright (with optional npx/pnpm prefix)
    if match_cmd == "playwright"
        || match_cmd.starts_with("playwright ")
        || match_cmd == "npx playwright"
        || match_cmd.starts_with("npx playwright ")
        || match_cmd == "pnpm playwright"
        || match_cmd.starts_with("pnpm playwright ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("pnpm ")
            .trim_start_matches("playwright")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk playwright".to_string()
        } else {
            format!("rtk playwright {}", rest)
        });
    }

    // prisma (with optional npx prefix)
    if match_cmd == "prisma"
        || match_cmd.starts_with("prisma ")
        || match_cmd == "npx prisma"
        || match_cmd.starts_with("npx prisma ")
    {
        let rest = match_cmd
            .trim_start_matches("npx ")
            .trim_start_matches("prisma")
            .trim_start();
        return Some(if rest.is_empty() {
            "rtk prisma".to_string()
        } else {
            format!("rtk prisma {}", rest)
        });
    }

    None
}

pub fn try_rewrite_pnpm_pkg(match_cmd: &str, cmd_body: &str) -> Option<String> {
    if !match_cmd.starts_with("pnpm ") {
        return None;
    }
    let after_pnpm = match_cmd.strip_prefix("pnpm ").unwrap_or("");
    let subcmd = after_pnpm.split_whitespace().next().unwrap_or("");
    match subcmd {
        "list" | "ls" | "outdated" => Some(replace_prefix(cmd_body, "pnpm ", "rtk pnpm ")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::rewrite_chain;

    fn rewrite(cmd: &str) -> Option<String> {
        if cmd.contains("<<") {
            return None;
        }
        rewrite_chain(cmd)
    }

    #[test]
    fn test_vitest() {
        assert_eq!(rewrite("vitest run"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_npx_vitest() {
        assert_eq!(rewrite("npx vitest"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_pnpm_test() {
        assert_eq!(rewrite("pnpm test"), Some("rtk vitest run".into()));
    }

    #[test]
    fn test_npm_test() {
        assert_eq!(rewrite("npm test"), Some("rtk npm test".into()));
    }

    #[test]
    fn test_npm_run() {
        assert_eq!(rewrite("npm run build"), Some("rtk npm build".into()));
    }

    #[test]
    fn test_tsc() {
        assert_eq!(rewrite("tsc --noEmit"), Some("rtk tsc --noEmit".into()));
    }

    #[test]
    fn test_npx_tsc() {
        assert_eq!(rewrite("npx tsc --noEmit"), Some("rtk tsc --noEmit".into()));
    }

    #[test]
    fn test_eslint() {
        assert_eq!(rewrite("eslint src/"), Some("rtk lint src/".into()));
    }

    #[test]
    fn test_pnpm_eslint() {
        assert_eq!(rewrite("pnpm eslint ."), Some("rtk lint .".into()));
    }

    #[test]
    fn test_pnpm_eslint_bare() {
        assert_eq!(rewrite("pnpm eslint"), Some("rtk lint".into()));
    }

    #[test]
    fn test_prettier() {
        assert_eq!(
            rewrite("prettier --check ."),
            Some("rtk prettier --check .".into())
        );
    }

    #[test]
    fn test_playwright() {
        assert_eq!(
            rewrite("npx playwright test"),
            Some("rtk playwright test".into())
        );
    }

    #[test]
    fn test_prisma() {
        assert_eq!(
            rewrite("npx prisma generate"),
            Some("rtk prisma generate".into())
        );
    }

    #[test]
    fn test_pnpm_list() {
        assert_eq!(rewrite("pnpm list"), Some("rtk pnpm list".into()));
    }

    #[test]
    fn test_pnpm_outdated() {
        assert_eq!(rewrite("pnpm outdated"), Some("rtk pnpm outdated".into()));
    }
}
