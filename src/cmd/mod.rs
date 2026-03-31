pub(crate) mod analysis;
pub(crate) mod predicates;

pub(crate) mod builtins;
pub(crate) mod filters;

pub mod exec;
pub mod hook;

// Re-export existing lexer from discover module
pub(crate) mod lexer {
    pub use crate::discover::lexer::*;
}

#[cfg(test)]
pub(crate) mod test_helpers;

pub use exec::execute;
pub use hook::check_for_hook;
