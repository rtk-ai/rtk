//! Local-only, opaque identifiers for RTK invocations and recall emissions.

use std::sync::OnceLock;

static INVOCATION_ID: OnceLock<Option<String>> = OnceLock::new();

pub fn new_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn current() -> Option<&'static str> {
    INVOCATION_ID.get_or_init(new_id).as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_id_stays_stable_and_event_ids_are_distinct() {
        let invocation = current().expect("invocation ID");
        assert_eq!(current(), Some(invocation));
        let event = new_id().expect("event ID");
        for id in [invocation, event.as_str()] {
            assert_eq!(id.len(), 32);
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
        assert_ne!(invocation, event);
    }
}
