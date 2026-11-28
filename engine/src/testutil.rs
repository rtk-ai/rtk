use crate::block::{Block, ContentFormat, Origin, Provenance, Subject};

pub static ORIGIN: Origin = Origin {
    tool: None,
    path: None,
    command: None,
    block_index: None,
};

pub static PROVENANCE: Provenance = Provenance {
    applied: Vec::new(),
};

pub fn block(text: &str, format: ContentFormat) -> Block<'_> {
    Block {
        text,
        subject: Subject::ToolResult,
        format,
        origin: &ORIGIN,
        provenance: &PROVENANCE,
    }
}
