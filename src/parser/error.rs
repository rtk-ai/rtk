/// Parser error types for structured output parsing
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("JSON parse failed at line {line}, column {col}: {msg}")]
    JsonError {
        line: usize,
        col: usize,
        msg: String,
    },

    #[error("Pattern mismatch: expected {expected}")]
    PatternMismatch { expected: &'static str },

    #[error("Partial parse: got {found}, missing fields: {missing:?}")]
    PartialParse {
        found: String,
        missing: Vec<&'static str>,
    },

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Missing required field: {0}")]
    MissingField(&'static str),

    #[error("Version mismatch: got {got}, expected {expected}")]
    VersionMismatch { got: String, expected: String },

    #[error("Empty output")]
    EmptyOutput,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<serde_json::Error> for ParseError {
    fn from(err: serde_json::Error) -> Self {
        ParseError::JsonError {
            line: err.line(),
            col: err.column(),
            msg: err.to_string(),
        }
    }
}
