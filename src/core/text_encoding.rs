use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TextEncoding {
    Auto,
    Utf8,
    Cp949,
    Latin1,
    #[value(name = "windows-1252")]
    Windows1252,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsedEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Cp949,
    Latin1,
    Windows1252,
}

impl UsedEncoding {
    pub fn label(self) -> &'static str {
        match self {
            UsedEncoding::Utf8 => "utf8",
            UsedEncoding::Utf16Le => "utf16-le",
            UsedEncoding::Utf16Be => "utf16-be",
            UsedEncoding::Cp949 => "cp949",
            UsedEncoding::Latin1 => "latin1",
            UsedEncoding::Windows1252 => "windows-1252",
        }
    }
}

pub struct DecodedText {
    pub text: String,
    pub used: UsedEncoding,
    /// True when `--encoding auto` selected a non-UTF8 fallback.
    pub used_fallback: bool,
}

pub fn decode_bytes(bytes: &[u8], requested: TextEncoding) -> Result<DecodedText> {
    // Always honor UTF-16 BOMs first (MSBuild logs on Windows).
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        return Ok(DecodedText {
            text: decode_utf16(&bytes[2..], true),
            used: UsedEncoding::Utf16Le,
            used_fallback: false,
        });
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        return Ok(DecodedText {
            text: decode_utf16(&bytes[2..], false),
            used: UsedEncoding::Utf16Be,
            used_fallback: false,
        });
    }

    let (payload, had_utf8_bom) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (&bytes[3..], true)
    } else {
        (bytes, false)
    };

    match requested {
        TextEncoding::Auto => {
            if let Ok(s) = std::str::from_utf8(payload) {
                return Ok(DecodedText {
                    text: s.to_string(),
                    used: UsedEncoding::Utf8,
                    used_fallback: false,
                });
            }

            // Windows-ish fallbacks first (common for legacy C++ source / logs).
            //
            // Note: WINDOWS-1252 decoding is permissive for all bytes, so try a
            // stricter multibyte encoding first when explicitly supported.
            for enc in [TextEncoding::Cp949, TextEncoding::Windows1252] {
                if let Ok(dt) = decode_bytes(payload, enc) {
                    return Ok(DecodedText {
                        text: if had_utf8_bom {
                            // Should not happen (UTF-8 BOM implies UTF-8), but keep behavior explicit.
                            dt.text
                        } else {
                            dt.text
                        },
                        used: dt.used,
                        used_fallback: true,
                    });
                }
            }

            // Last resort: byte-safe 1:1 mapping.
            let dt = decode_bytes(payload, TextEncoding::Latin1)?;
            Ok(DecodedText {
                text: dt.text,
                used: dt.used,
                used_fallback: true,
            })
        }
        TextEncoding::Utf8 => Ok(DecodedText {
            text: String::from_utf8(payload.to_vec())
                .context("stream did not contain valid UTF-8")?,
            used: UsedEncoding::Utf8,
            used_fallback: false,
        }),
        TextEncoding::Windows1252 => {
            let (cow, _, had_errors) = encoding_rs::WINDOWS_1252.decode(payload);
            if had_errors {
                return Err(anyhow!("invalid bytes for windows-1252"));
            }
            Ok(DecodedText {
                text: cow.into_owned(),
                used: UsedEncoding::Windows1252,
                used_fallback: false,
            })
        }
        TextEncoding::Cp949 => {
            let (cow, _, had_errors) = encoding_rs::EUC_KR.decode(payload);
            if had_errors {
                return Err(anyhow!("invalid bytes for cp949"));
            }
            Ok(DecodedText {
                text: cow.into_owned(),
                used: UsedEncoding::Cp949,
                used_fallback: false,
            })
        }
        TextEncoding::Latin1 => {
            let text: String = payload.iter().map(|b| *b as char).collect();
            Ok(DecodedText {
                text,
                used: UsedEncoding::Latin1,
                used_fallback: false,
            })
        }
    }
}

pub fn encode_text(text: &str, encoding: UsedEncoding) -> Result<Vec<u8>> {
    match encoding {
        UsedEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        UsedEncoding::Utf16Le => {
            // Keep it simple: patch currently does not target UTF-16 paths.
            Err(anyhow!("encoding utf16-le output is not supported"))
        }
        UsedEncoding::Utf16Be => Err(anyhow!("encoding utf16-be output is not supported")),
        UsedEncoding::Windows1252 => {
            let (cow, _, had_errors) = encoding_rs::WINDOWS_1252.encode(text);
            if had_errors {
                return Err(anyhow!("text not representable in windows-1252"));
            }
            Ok(cow.into_owned())
        }
        UsedEncoding::Cp949 => {
            let (cow, _, had_errors) = encoding_rs::EUC_KR.encode(text);
            if had_errors {
                return Err(anyhow!("text not representable in cp949"));
            }
            Ok(cow.into_owned())
        }
        UsedEncoding::Latin1 => {
            let mut out = Vec::with_capacity(text.len());
            for ch in text.chars() {
                let u = ch as u32;
                if u > 0xFF {
                    return Err(anyhow!("text not representable in latin1"));
                }
                out.push(u as u8);
            }
            Ok(out)
        }
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}
