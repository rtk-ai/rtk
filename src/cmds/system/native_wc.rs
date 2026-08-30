//! Pure-Rust native wc implementation.
//!
//! Provides a cross-platform `wc` command replacement that doesn't rely on
//! external binaries (especially important on Windows where GNU wc is missing).

use anyhow::Result;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

/// Configuration for the native wc command.
#[derive(Debug, Clone)]
pub struct WcConfig {
    /// Count lines
    pub count_lines: bool,
    /// Count words
    pub count_words: bool,
    /// Count bytes
    pub count_bytes: bool,
    /// Count characters
    pub count_chars: bool,
    /// Files to process (empty = stdin)
    pub files: Vec<PathBuf>,
}

impl Default for WcConfig {
    fn default() -> Self {
        Self {
            count_lines: true,
            count_words: true,
            count_bytes: true,
            count_chars: false,
            files: Vec::new(),
        }
    }
}

/// Results for a single file.
#[derive(Debug)]
struct WcResult {
    lines: usize,
    words: usize,
    bytes: usize,
    chars: usize,
    file: Option<PathBuf>,
}

impl WcResult {
    fn new() -> Self {
        Self {
            lines: 0,
            words: 0,
            bytes: 0,
            chars: 0,
            file: None,
        }
    }

    fn add(&mut self, other: &WcResult) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
        self.chars += other.chars;
    }

    fn format(&self, config: &WcConfig) -> String {
        let mut parts = Vec::new();
        
        if config.count_lines {
            parts.push(self.lines.to_string());
        }
        if config.count_words {
            parts.push(self.words.to_string());
        }
        if config.count_bytes {
            parts.push(self.bytes.to_string());
        }
        if config.count_chars {
            parts.push(self.chars.to_string());
        }
        
        if let Some(file) = &self.file {
            parts.push(file.to_string_lossy().to_string());
        }
        
        parts.join(" ")
    }
}

/// Count lines, words, bytes, and chars from a reader.
fn count_from_reader<R: Read>(mut reader: R) -> Result<WcResult> {
    let mut result = WcResult::new();
    let mut buffer = String::new();
    let mut bytes_read = 0;
    
    // Read in chunks to handle large files efficiently
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes_read += n;

        // Convert bytes to string, handling UTF-8 boundaries
        let chunk = String::from_utf8_lossy(&buf[..n]);
        buffer.push_str(&chunk);

        // Count only the lines this chunk completed; whatever follows the last
        // newline is an unterminated tail that the next chunk continues.
        // `split_terminator` keeps any `\r` in the line, matching what `wc`
        // counts, and does not emit a phantom empty line after the final `\n`.
        let Some(last_newline) = buffer.rfind('\n') else {
            continue;
        };
        let tail_start = last_newline + 1;
        for line in buffer[..tail_start].split_terminator('\n') {
            result.lines += 1;
            result.words += line.split_whitespace().count();
            result.chars += line.chars().count() + 1; // +1 for newline
        }
        // The borrow above ends here, so the counted prefix can be dropped.
        buffer.drain(..tail_start);
    }

    // Handle last partial line (if no trailing newline)
    if !buffer.is_empty() {
        result.lines += 1;
        result.words += buffer.split_whitespace().count();
        result.chars += buffer.chars().count();
    }
    
    result.bytes = bytes_read;
    Ok(result)
}

/// Process a single file.
fn process_file(path: &PathBuf) -> Result<WcResult> {
    let file = fs::File::open(path)?;
    let mut result = count_from_reader(file)?;
    result.file = Some(path.clone());
    Ok(result)
}

/// Process stdin.
fn process_stdin() -> Result<WcResult> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    count_from_reader(reader)
}

/// Parse `wc` flags into a [`WcConfig`].
///
/// Flags are additive, the way `wc` itself treats them: `wc -lw` reports lines
/// *and* words. (Each flag used to clear the others, so `-lw` reported words
/// only.) With no counter flag at all, `wc` prints lines, words and bytes.
fn parse_wc_config(args: &[String], verbose: u8) -> WcConfig {
    let mut config = WcConfig {
        count_lines: false,
        count_words: false,
        count_bytes: false,
        count_chars: false,
        files: Vec::new(),
    };

    for arg in args {
        match arg.as_str() {
            "-l" | "--lines" => config.count_lines = true,
            "-w" | "--words" => config.count_words = true,
            "-c" | "--bytes" => config.count_bytes = true,
            "-m" | "--chars" => config.count_chars = true,
            "-L" | "--max-line-length" => {
                if verbose > 0 {
                    eprintln!("Warning: -L/--max-line-length not supported in native wc");
                }
            }
            // Bundled short flags, e.g. `-lw`.
            _ if arg.starts_with('-') => {
                for ch in arg.chars().skip(1) {
                    match ch {
                        'l' => config.count_lines = true,
                        'w' => config.count_words = true,
                        'c' => config.count_bytes = true,
                        'm' => config.count_chars = true,
                        _ => {}
                    }
                }
            }
            _ => config.files.push(PathBuf::from(arg)),
        }
    }

    if !config.count_lines && !config.count_words && !config.count_bytes && !config.count_chars {
        config.count_lines = true;
        config.count_words = true;
        config.count_bytes = true;
    }
    config
}

/// Run the native wc command.
pub fn run_native_wc(args: &[String], verbose: u8) -> Result<i32> {
    let config = parse_wc_config(args, verbose);

    let mut results = Vec::new();
    let mut total = WcResult::new();
    
    if config.files.is_empty() {
        // Read from stdin
        let result = process_stdin()?;
        total.add(&result);
        results.push(result);
    } else {
        // Process each file
        for file in &config.files {
            let result = process_file(file)?;
            total.add(&result);
            results.push(result);
        }
        
        // Add total if multiple files
        if config.files.len() > 1 {
            total.file = Some(PathBuf::from("total"));
            results.push(total);
        }
    }
    
    // Output results
    for result in &results {
        println!("{}", result.format(&config));
    }
    
    if verbose > 0 {
        eprintln!("Native wc processed {} file(s)", config.files.len().max(1));
    }
    
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_wc_result_format() {
        let config = WcConfig {
            count_lines: true,
            count_words: true,
            count_bytes: true,
            count_chars: false,
            files: vec![],
        };
        
        let mut result = WcResult::new();
        result.lines = 10;
        result.words = 50;
        result.bytes = 300;
        result.file = Some(PathBuf::from("test.txt"));
        
        let formatted = result.format(&config);
        assert_eq!(formatted, "10 50 300 test.txt");
    }

    #[test]
    fn test_wc_result_format_lines_only() {
        let config = WcConfig {
            count_lines: true,
            count_words: false,
            count_bytes: false,
            count_chars: false,
            files: vec![],
        };
        
        let mut result = WcResult::new();
        result.lines = 10;
        result.file = Some(PathBuf::from("test.txt"));
        
        let formatted = result.format(&config);
        assert_eq!(formatted, "10 test.txt");
    }

    #[test]
    fn test_wc_result_format_no_file() {
        let config = WcConfig {
            count_lines: true,
            count_words: true,
            count_bytes: true,
            count_chars: false,
            files: vec![],
        };
        
        let mut result = WcResult::new();
        result.lines = 10;
        result.words = 50;
        result.bytes = 300;
        
        let formatted = result.format(&config);
        assert_eq!(formatted, "10 50 300");
    }

    #[test]
    fn test_native_wc_stdin() {
        // This test is harder to do in isolation since it reads from stdin
        // We'll test the counting logic instead
    }

    #[test]
    fn test_native_wc_file() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "hello world").unwrap();
        writeln!(file, "foo bar baz").unwrap();
        writeln!(file, "last line").unwrap();
        drop(file);
        
        let result = process_file(&file_path).unwrap();
        assert_eq!(result.lines, 3);
        // "hello world" (2) + "foo bar baz" (3) + "last line" (2)
        assert_eq!(result.words, 7);
        assert!(result.bytes > 0);
    }

    #[test]
    fn test_combined_flags() {
        let args = vec!["-lw".to_string(), "test.txt".to_string()];
        let config = parse_wc_config(&args, 0);
        assert!(config.count_lines);
        assert!(config.count_words);
        assert!(!config.count_bytes);
        assert!(!config.count_chars);
    }
    
}