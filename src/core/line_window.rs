use std::collections::VecDeque;
use std::io::{BufRead, Result, Write};

pub fn write_head<R: BufRead, W: Write>(mut reader: R, mut writer: W, lines: usize) -> Result<()> {
    if lines == 0 {
        return Ok(());
    }

    let mut line = String::new();
    for _ in 0..lines {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        writer.write_all(line.as_bytes())?;
    }
    Ok(())
}

pub fn write_tail<R: BufRead, W: Write>(mut reader: R, mut writer: W, lines: usize) -> Result<()> {
    if lines == 0 {
        return Ok(());
    }

    let mut ring = VecDeque::with_capacity(lines);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        if ring.len() == lines {
            ring.pop_front();
        }
        ring.push_back(line.clone());
    }

    for line in ring {
        writer.write_all(line.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn head_writes_exact_first_lines() {
        let input = Cursor::new("a\nb\nc\n");
        let mut out = Vec::new();
        write_head(input, &mut out, 2).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a\nb\n");
    }

    #[test]
    fn head_zero_writes_nothing() {
        let input = Cursor::new("a\nb\n");
        let mut out = Vec::new();
        write_head(input, &mut out, 0).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn tail_writes_exact_last_lines() {
        let input = Cursor::new("a\nb\nc\n");
        let mut out = Vec::new();
        write_tail(input, &mut out, 2).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "b\nc\n");
    }

    #[test]
    fn tail_zero_writes_nothing() {
        let input = Cursor::new("a\nb\n");
        let mut out = Vec::new();
        write_tail(input, &mut out, 0).unwrap();
        assert!(out.is_empty());
    }
}
