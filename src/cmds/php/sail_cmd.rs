//! Laravel Sail wrapper: filters `sail artisan test` and `sail test`, passes every other
//! Sail command through untouched.

use super::artisan_cmd::{ArtisanCall, filter_artisan_call, parse_artisan_call};
use super::utils::{detect_php_test_runner, php_tool_command};
use crate::core::runner;
use crate::core::shell::display_args;
use anyhow::Result;

/// Returns the artisan call when `sail <args>` runs `artisan test`, spelled in any form Sail accepts.
fn test_call(args: &[String]) -> Option<ArtisanCall> {
    let word = |i: usize| args.get(i).map(String::as_str);
    let rest = match (word(0), word(1)) {
        (Some("artisan" | "art" | "a"), _) => &args[1..],
        (Some("php"), Some("artisan")) => &args[2..],
        // `sail test` is Sail's alias for `artisan test`.
        (Some("test"), _) => args,
        _ => return None,
    };
    Some(parse_artisan_call(rest)).filter(ArtisanCall::is_test)
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("sail");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: sail {}", args.join(" "));
    }

    let label = display_args(args);
    match test_call(args) {
        Some(call) => {
            let test_runner = detect_php_test_runner();
            runner::run_filtered(
                cmd,
                "sail",
                &label,
                move |raw| filter_artisan_call(&call, test_runner, raw),
                runner::RunOptions::with_tee("artisan-test"),
            )
        }
        None => runner::run(
            cmd,
            "sail",
            &label,
            runner::RunMode::Passthrough,
            runner::RunOptions::default(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn artisan_test_spellings_are_recognized() {
        for words in [
            &["artisan", "test"][..],
            &["art", "test", "--filter=Foo"],
            &["a", "--env", "testing", "test"],
            &["php", "artisan", "test"],
            &["test", "--parallel"],
        ] {
            assert!(test_call(&args(words)).is_some(), "{words:?}");
        }
    }

    #[test]
    fn other_sail_commands_pass_through() {
        for words in [
            &["up", "-d"][..],
            &["artisan", "serve"],
            &["artisan", "migrate"],
            &["exec", "pgsql", "psql"],
            &["composer", "install"],
            &["php", "-v"],
            &[],
        ] {
            assert!(test_call(&args(words)).is_none(), "{words:?}");
        }
    }
}
