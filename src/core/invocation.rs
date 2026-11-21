//! Local-only identifiers for hook decisions, RTK processes and output captures.
//! IDs are opaque random values, never derived from command text or timestamps.

use std::cell::RefCell;

#[derive(Clone, Default)]
pub struct Invocation {
    pub id: Option<String>,
    pub hook_event_id: Option<String>,
}

thread_local! {
    static CURRENT: RefCell<Option<Invocation>> = const { RefCell::new(None) };
}

pub fn new_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn initialize(hook_event_id: Option<&str>) {
    CURRENT.with(|current| {
        *current.borrow_mut() = Some(Invocation {
            id: new_id(),
            hook_event_id: hook_event_id.filter(|id| valid_id(id)).map(str::to_owned),
        });
    });
}

pub fn current() -> Invocation {
    CURRENT.with(|current| {
        current
            .borrow_mut()
            .get_or_insert_with(|| Invocation {
                id: new_id(),
                hook_event_id: None,
            })
            .clone()
    })
}

/// Annotate only a proven simple, leading RTK command. Never rewrite quoted
/// operands, shell wrappers, pipelines or compound commands to carry metadata.
/// Those retain unknown attribution until their rewrite paths support it.
pub fn annotate_rewrite(command: &str, event_id: &str) -> Option<String> {
    use crate::discover::lexer::{TokenKind, tokenize_with_newlines};
    if !valid_id(event_id) || !command.starts_with("rtk ") {
        return None;
    }
    if tokenize_with_newlines(command)
        .iter()
        .any(|token| !matches!(token.kind, TokenKind::Arg | TokenKind::Redirect))
    {
        return None;
    }
    Some(format!("rtk --hook-event-id {event_id} {}", &command[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_and_bounded() {
        let a = new_id().expect("random ID");
        let b = new_id().expect("random ID");
        assert!(valid_id(&a));
        assert_ne!(a, b);
        assert!(!valid_id("../untrusted"));
        assert!(!valid_id(&"a".repeat(33)));
    }

    #[test]
    fn annotation_preserves_operands_and_defers_shell_shapes() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            annotate_rewrite("rtk git log --format='rtk && text'", id),
            Some(format!(
                "rtk --hook-event-id {id} git log --format='rtk && text'"
            ))
        );
        for command in [
            "rtk git status && rtk git log",
            "rtk ls | cat",
            "rtk ls\nrtk ls",
            "A=1 rtk ls",
            "env A=1 rtk ls",
            "sh -c 'rtk ls'",
            "rtk ls $(pwd)",
        ] {
            assert!(annotate_rewrite(command, id).is_none(), "{command}");
        }
    }
}
