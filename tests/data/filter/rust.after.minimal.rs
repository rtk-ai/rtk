use std::fmt;

/**
 * Doc block kept by minimal.
 */

pub const MAX: usize = 100;

pub struct Greeting {
    name: String,
}

pub(crate) struct Internal {
    value: u32,
}

/// Doc comment kept by minimal.
pub fn greet(name: &str) -> String {
    format!("Hello, {}", name)
}

pub(crate) fn helper(name: &str) -> String {
    format!("Help, {}", name)
}

pub fn farewell(name: &str) -> String {
    format!("Goodbye, {}", name)
}

fn private_fn(name: &str) -> String {
    format!("Private, {}", name)
}

#[test]
fn test_farewell() {
    assert_eq!(farewell("World"), "Goodbye, World");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greet() {
        assert_eq!(greet("World"), "Hello, World");
    }
}
