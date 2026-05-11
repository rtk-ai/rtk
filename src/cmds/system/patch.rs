use crate::core::text_encoding::{self, TextEncoding, UsedEncoding};
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub struct PatchArgs<'a> {
    pub file: &'a Path,
    pub encoding: TextEncoding,
    pub old: &'a str,
    pub new: &'a str,
    pub all: bool,
    pub backup: bool,
}

pub fn run(args: PatchArgs<'_>, verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("rtk patch: {}", args.file.display());
    }

    let original = fs::read(args.file)
        .with_context(|| format!("Failed to read file: {}", args.file.display()))?;

    let decoded = text_encoding::decode_bytes(&original, args.encoding)
        .with_context(|| format!("Failed to decode file: {}", args.file.display()))?;

    if matches!(decoded.used, UsedEncoding::Utf16Le | UsedEncoding::Utf16Be) {
        return Err(anyhow!(
            "utf16 input is not supported by rtk patch (use a UTF-8/ANSI file)"
        ));
    }

    if decoded.used_fallback {
        eprintln!(
            "rtk patch: decoded {} as {}",
            args.file.display(),
            decoded.used.label()
        );
    }

    let count = decoded.text.match_indices(args.old).count();
    if count == 0 {
        return Err(anyhow!("no matches for --replace in {}", args.file.display()));
    }
    if !args.all && count != 1 {
        return Err(anyhow!(
            "expected exactly 1 match for --replace (found {}); pass --all to replace all",
            count
        ));
    }

    let replaced = if args.all {
        decoded.text.replace(args.old, args.new)
    } else {
        decoded.text.replacen(args.old, args.new, 1)
    };

    let out = text_encoding::encode_text(&replaced, decoded.used)
        .context("Failed to encode patched content")?;

    if args.backup {
        let backup_path = bak_path(args.file);
        fs::write(&backup_path, &original).with_context(|| {
            format!(
                "Failed to write backup file: {}",
                backup_path.display()
            )
        })?;
    }

    fs::write(args.file, out)
        .with_context(|| format!("Failed to write file: {}", args.file.display()))?;

    Ok(0)
}

fn bak_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(format!("{}.bak", s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text_encoding::TextEncoding;
    use tempfile::NamedTempFile;

    #[test]
    fn test_patch_preserves_non_utf8_bytes_latin1() -> Result<()> {
        let f = NamedTempFile::new()?;
        // Contains non-UTF8 bytes (0xFF, 0xFE) that must survive unchanged.
        let original: Vec<u8> = b"AA OLD BB\n".iter().copied().chain([0xFF, 0xFE]).collect();
        fs::write(f.path(), &original)?;

        run(
            PatchArgs {
                file: f.path(),
                encoding: TextEncoding::Latin1,
                old: "OLD",
                new: "NEW",
                all: false,
                backup: false,
            },
            0,
        )?;

        let out = fs::read(f.path())?;
        assert!(out.starts_with(b"AA NEW BB\n"));
        assert_eq!(&out[out.len() - 2..], &[0xFF, 0xFE]);
        Ok(())
    }
}
