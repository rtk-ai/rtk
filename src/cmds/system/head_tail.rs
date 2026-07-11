use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

use crate::core::line_window;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Head,
    Tail,
}

#[derive(Debug, PartialEq, Eq)]
struct Spec {
    lines: usize,
    files: Vec<PathBuf>,
}

pub fn run_head(args: &[String]) -> Result<i32> {
    let spec = parse(Mode::Head, args)?;
    run_spec(Mode::Head, spec)
}

pub fn run_tail(args: &[String]) -> Result<i32> {
    let spec = parse(Mode::Tail, args)?;
    run_spec(Mode::Tail, spec)
}

fn run_spec(mode: Mode, spec: Spec) -> Result<i32> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    run_with_writer(mode, &spec, &mut output)
}

fn run_with_writer<W: Write>(mode: Mode, spec: &Spec, writer: &mut W) -> Result<i32> {
    let multiple_files = spec.files.len() > 1;
    let mut exit_code = 0;

    for (index, path) in spec.files.iter().enumerate() {
        if multiple_files {
            if index > 0 {
                writeln!(writer)?;
            }
            writeln!(writer, "==> {} <==", path.display())?;
        }

        let result = if path == std::path::Path::new("-") {
            let stdin = io::stdin();
            let input = stdin.lock();
            write_window(mode, input, &mut *writer, spec.lines)
        } else {
            match File::open(path) {
                Ok(file) => write_window(mode, BufReader::new(file), &mut *writer, spec.lines),
                Err(err) => {
                    eprintln!("rtk {}: cannot open {}: {err}", mode.name(), path.display());
                    exit_code = 1;
                    continue;
                }
            }
        };

        if let Err(err) = result {
            eprintln!("rtk {}: cannot read {}: {err}", mode.name(), path.display());
            exit_code = 1;
        }
    }

    Ok(exit_code)
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Tail => "tail",
        }
    }
}

fn write_window<R: io::BufRead, W: Write>(
    mode: Mode,
    reader: R,
    writer: W,
    lines: usize,
) -> io::Result<()> {
    match mode {
        Mode::Head => line_window::write_head(reader, writer, lines),
        Mode::Tail => line_window::write_tail(reader, writer, lines),
    }
}

fn parse(mode: Mode, args: &[String]) -> Result<Spec> {
    let mut lines = 10usize;
    let mut files = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        if token == "-c" || token.starts_with("-c") || token.starts_with("--bytes") {
            return Err(anyhow!("byte counts are unsupported"));
        }
        if mode == Mode::Tail && (token == "-f" || token == "--follow" || token.starts_with("--follow=")) {
            return Err(anyhow!("rtk tail: follow mode is unsupported"));
        }

        if token == "-n" || token == "--lines" {
            lines = parse_count(args.get(i + 1), token)?;
            i += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--lines=") {
            lines = parse_count(Some(value), "--lines")?;
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("-n") {
            if !value.is_empty() {
                lines = parse_count(Some(value), "-n")?;
                i += 1;
                continue;
            }
        }
        if token.starts_with('-') && token.len() > 1 && token[1..].chars().all(|c| c.is_ascii_digit()) {
            lines = parse_count(Some(&token[1..]), token)?;
            i += 1;
            continue;
        }
        if token.starts_with('-') && token != "-" {
            return Err(anyhow!("unsupported option: {token}"));
        }

        files.push(PathBuf::from(token));
        i += 1;
    }

    if files.is_empty() {
        return Err(anyhow!("usage: rtk {} [OPTION]... FILE...", mode.name()));
    }

    Ok(Spec {
        lines,
        files,
    })
}

fn parse_count(value: Option<impl AsRef<str>>, flag: &str) -> Result<usize> {
    let value = value
        .ok_or_else(|| anyhow!("{flag} requires a line count"))?
        .as_ref()
        .to_string();
    if value.starts_with('-') {
        return Err(anyhow!("negative line counts are unsupported"));
    }
    value
        .parse::<usize>()
        .with_context(|| format!("invalid line count for {flag}: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_head_default() {
        assert_eq!(
            parse(Mode::Head, &["foo.txt".to_string()]).unwrap(),
            Spec {
                lines: 10,
                files: vec![PathBuf::from("foo.txt")],
            }
        );
    }

    #[test]
    fn parse_head_dash_n() {
        assert_eq!(
            parse(Mode::Head, &["-n".to_string(), "20".to_string(), "foo.txt".to_string()])
                .unwrap()
                .lines,
            20
        );
    }

    #[test]
    fn parse_head_compact_dash_number() {
        assert_eq!(
            parse(Mode::Head, &["-20".to_string(), "foo.txt".to_string()])
                .unwrap()
                .lines,
            20
        );
    }

    #[test]
    fn parse_head_accepts_multiple_files() {
        assert_eq!(
            parse(Mode::Head, &["a".to_string(), "b".to_string()])
                .unwrap()
                .files,
            vec![PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    #[test]
    fn parse_head_without_a_file_returns_usage() {
        assert!(parse(Mode::Head, &[]).unwrap_err().to_string().starts_with("usage:"));
    }

    #[test]
    fn parse_tail_default() {
        assert_eq!(
            parse(Mode::Tail, &["foo.txt".to_string()]).unwrap(),
            Spec {
                lines: 10,
                files: vec![PathBuf::from("foo.txt")],
            }
        );
    }

    #[test]
    fn parse_tail_dash_n() {
        assert_eq!(
            parse(Mode::Tail, &["-n".to_string(), "20".to_string(), "foo.txt".to_string()])
                .unwrap()
                .lines,
            20
        );
    }

    #[test]
    fn parse_tail_follow_rejected() {
        let err = parse(Mode::Tail, &["-f".to_string(), "foo.txt".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("follow mode"));
    }

    #[test]
    fn parse_tail_accepts_multiple_files() {
        assert_eq!(
            parse(Mode::Tail, &["a".to_string(), "b".to_string()])
                .unwrap()
                .files,
            vec![PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    #[test]
    fn head_output_has_no_omission_marker() {
        let mut out = Vec::new();
        write_window(Mode::Head, Cursor::new("a\nb\nc\n"), &mut out, 2).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "a\nb\n");
        assert!(!text.contains("omitted"));
    }

    #[test]
    fn tail_output_has_no_omission_marker() {
        let mut out = Vec::new();
        write_window(Mode::Tail, Cursor::new("a\nb\nc\n"), &mut out, 2).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "b\nc\n");
        assert!(!text.contains("omitted"));
    }

    #[test]
    fn head_empty_file_is_empty() {
        let mut out = Vec::new();
        write_window(Mode::Head, Cursor::new(""), &mut out, 10).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn tail_empty_file_is_empty() {
        let mut out = Vec::new();
        write_window(Mode::Tail, Cursor::new(""), &mut out, 10).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn tail_zero_writes_nothing() {
        let mut out = Vec::new();
        write_window(Mode::Tail, Cursor::new("a\nb\n"), &mut out, 0).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn run_with_writer_adds_headers_and_blank_lines_for_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, "first\nignored\n").unwrap();
        std::fs::write(&second, "second\nignored\n").unwrap();
        let spec = Spec {
            lines: 1,
            files: vec![first.clone(), second.clone()],
        };
        let mut output = Vec::new();

        assert_eq!(run_with_writer(Mode::Head, &spec, &mut output).unwrap(), 0);

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "==> {} <==\nfirst\n\n==> {} <==\nsecond\n",
                first.display(),
                second.display()
            )
        );
    }

    #[test]
    fn run_with_writer_continues_after_a_file_failure() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        let readable = dir.path().join("readable.txt");
        std::fs::write(&readable, "available\n").unwrap();
        let spec = Spec {
            lines: 10,
            files: vec![missing.clone(), readable.clone()],
        };
        let mut output = Vec::new();

        assert_eq!(run_with_writer(Mode::Head, &spec, &mut output).unwrap(), 1);

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "==> {} <==\n\n==> {} <==\navailable\n",
                missing.display(),
                readable.display()
            )
        );
    }
}
