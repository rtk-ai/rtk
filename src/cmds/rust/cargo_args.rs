//! Locate Cargo's subcommand without losing rustup selectors or global options.

use std::ffi::OsStr;

#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub subcommand_index: usize,
    pub subcommand: &'static str,
}

/// Shared by shell classification and execution. Only consume documented global
/// options: an unknown option, missing value, or special operation stays native.
pub fn parse(args: &[impl AsRef<OsStr>]) -> Option<Invocation> {
    let mut index = 0;
    if args.first()?.as_ref().to_str()?.starts_with('+') {
        if args[0].as_ref().to_str()?.len() == 1 {
            return None;
        }
        index += 1;
    }
    loop {
        let arg = args.get(index)?.as_ref().to_str()?;
        match arg {
            "--locked" | "--offline" | "--frozen" | "--quiet" | "--verbose" => {}
            "--config" | "--color" | "-C" | "-Z" => {
                index += 1;
                let value = args.get(index)?.as_ref();
                // -Zscript changes the grammar: the next positional is a Rust
                // source file, even when its name happens to be `check`.
                if arg == "-Z" && value.to_str()?.split('=').next() == Some("script") {
                    return None;
                }
            }
            _ if arg.starts_with("--config=") || arg.starts_with("--color=") => {
                if arg.split_once('=')?.1.is_empty() {
                    return None;
                }
            }
            _ if arg.starts_with("-C") && arg.len() > 2 => {}
            _ if arg.starts_with("-Z") && arg.len() > 2 => {
                if arg[2..].split('=').next() == Some("script") {
                    return None;
                }
            }
            _ if arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].bytes().all(|b| matches!(b, b'v' | b'q')) => {}
            _ => {
                let subcommand = match arg {
                    "build" | "b" => "build",
                    "check" | "c" => "check",
                    "test" | "t" => "test",
                    "clippy" => "clippy",
                    "install" => "install",
                    "nextest" => "nextest",
                    "fmt" => "fmt",
                    _ => return None,
                };
                return Some(Invocation {
                    subcommand_index: index,
                    subcommand,
                });
            }
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_option_values_and_resolves_aliases() {
        for (args, index, name) in [
            (vec!["--config", "check", "t"], 2, "test"),
            (
                vec!["+nightly", "-Z", "unstable-options", "-C", "some path", "c"],
                5,
                "check",
            ),
            (
                vec![
                    "--config=a=1",
                    "--config",
                    "b=2",
                    "--color=never",
                    "--locked",
                    "--offline",
                    "--frozen",
                    "-vv",
                    "b",
                ],
                8,
                "build",
            ),
            (
                vec!["-Zunstable-options", "-Csome path", "-q", "clippy"],
                3,
                "clippy",
            ),
        ] {
            assert_eq!(
                parse(&args),
                Some(Invocation {
                    subcommand_index: index,
                    subcommand: name
                }),
                "{args:?}"
            );
        }
    }

    #[test]
    fn unknown_flags_and_special_operations_stay_native() {
        for args in [
            vec!["--config"],
            vec!["--color="],
            vec!["--future-option", "check"],
            vec!["+stable", "--help"],
            vec!["--version"],
            vec!["--list"],
            vec!["--explain", "check"],
            vec!["-Zscript", "check"],
            vec!["+nightly", "-Z", "script", "check"],
            vec!["metadata"],
            vec!["+", "check"],
            vec!["--", "check"],
            vec!["--config", "check"],
        ] {
            assert_eq!(parse(&args), None, "{args:?}");
        }
    }
}
