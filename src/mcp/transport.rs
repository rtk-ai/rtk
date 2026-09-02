use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};

pub struct Transport {
    reader: BufReader<std::io::Stdin>,
}

impl Transport {
    pub fn new() -> Self {
        Self {
            reader: BufReader::new(std::io::stdin()),
        }
    }

    /// Read one newline-terminated JSON line. Returns `None` on EOF.
    pub fn read_line(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .context("reading from stdin")?;
        if n == 0 {
            return Ok(None);
        }
        // Strip trailing CR/LF
        let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
        Ok(Some(trimmed))
    }

    /// Write one JSON line to stdout and flush immediately.
    pub fn write_response(&self, json: &str) -> Result<()> {
        let mut out = std::io::stdout().lock();
        out.write_all(json.as_bytes())
            .context("writing to stdout")?;
        out.write_all(b"\n").context("writing newline")?;
        out.flush().context("flushing stdout")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Transport uses real stdin/stdout so unit tests cover the helpers.

    #[test]
    fn test_trim_crlf() {
        let line = "hello\r\n";
        let trimmed = line.trim_end_matches(['\n', '\r']);
        assert_eq!(trimmed, "hello");
    }

    #[test]
    fn test_trim_lf_only() {
        let line = "hello\n";
        let trimmed = line.trim_end_matches(['\n', '\r']);
        assert_eq!(trimmed, "hello");
    }

    #[test]
    fn test_trim_no_newline() {
        let line = "hello";
        let trimmed = line.trim_end_matches(['\n', '\r']);
        assert_eq!(trimmed, "hello");
    }
}
