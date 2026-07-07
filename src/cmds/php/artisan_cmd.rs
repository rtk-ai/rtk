//! Laravel Artisan output cleanup helpers.

use super::test_output::filter_test_runner_output;
use super::utils::{strip_ansi_and_controls, PhpTestRunner};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref BOX_CHARS_RE: Regex =
        Regex::new(r"[\u{2500}-\u{257F}\u{2580}-\u{259F}\u{25A0}-\u{25FF}\u{27A0}-\u{27BF}]+")
            .unwrap();
    static ref DOTS_RE: Regex = Regex::new(r"\.{3,}").unwrap();
    static ref MULTI_SPACE_RE: Regex = Regex::new(r"[ \t]{2,}").unwrap();
    static ref MULTI_BLANK_RE: Regex = Regex::new(r"\n{3,}").unwrap();
}

pub fn filter_artisan_output(output: &str) -> String {
    let mut cleaned = strip_ansi_and_controls(output);
    cleaned = BOX_CHARS_RE.replace_all(&cleaned, "").to_string();
    cleaned = DOTS_RE.replace_all(&cleaned, "..").to_string();
    cleaned = MULTI_SPACE_RE.replace_all(&cleaned, " ").to_string();
    cleaned = MULTI_BLANK_RE.replace_all(&cleaned, "\n\n").to_string();
    cleaned.trim().to_string()
}

pub fn filter_artisan_test_output(output: &str, runner: PhpTestRunner) -> String {
    match runner {
        PhpTestRunner::Pest | PhpTestRunner::Phpunit => filter_test_runner_output(output),
        PhpTestRunner::Unknown => filter_artisan_output(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artisan_cleanup() {
        let out =
            "\u{1b}[32mEnvironment .....\u{1b}[0m\n\u{2502} Laravel Version \u{2502} 13.0.0 \u{2502}\n\n\n";
        let filtered = filter_artisan_output(out);
        assert!(!filtered.contains('\u{1b}'));
        assert!(!filtered.contains('\u{2502}'));
        assert!(filtered.contains("Environment .."));
    }

    #[test]
    fn test_artisan_test_prefers_runner_filter() {
        let output = "PHPUnit 12.2.0\n....\nOK (4 tests, 4 assertions)\n";
        let filtered = filter_artisan_test_output(output, PhpTestRunner::Phpunit);
        assert!(!filtered.contains("PHPUnit 12.2.0"));
        assert!(filtered.contains("OK (4 tests, 4 assertions)"));
    }
}
