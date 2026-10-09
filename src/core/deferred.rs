//! Warnings a caller may want to print itself, later and in an order of its
//! own. [`warn`] prints at once, as `eprintln!` does, unless a [`capture`] is
//! running on this thread, which holds them back and hands them over when its
//! closure returns.
//!
//! Used by the code that warns while a command is being rewritten
//! (configuration and filter-trust problems), so the rewrite can put its own
//! warning in front of them without knowing who else might speak.

use std::cell::RefCell;
use std::fmt::Arguments;

thread_local! {
    static HELD: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Prints `message` on stderr, or holds it if a [`capture`] is running.
pub fn warn(message: Arguments<'_>) {
    let held = HELD.with(|held| match held.borrow_mut().as_mut() {
        Some(list) => {
            list.push(message.to_string());
            true
        }
        None => false,
    });
    if !held {
        eprintln!("{message}");
    }
}

/// Runs `f`, holding back every [`warn`] it makes, and returns them in the
/// order they were made. A nested capture keeps its own.
///
/// What was held before `f` ran is put back when `f` returns and when it
/// unwinds, so a panic caught above never leaves this thread holding warnings
/// no capture will hand over. The warnings `f` made before it panicked are
/// dropped with it: they reach neither stderr nor an enclosing capture.
/// (Release builds abort on a panic, so this is only ever seen by tests.)
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let mut restore = Restore {
        outer: Some(HELD.with(|held| held.borrow_mut().replace(Vec::new()))),
    };
    let result = f();
    let mine = restore.now();
    (result, mine.unwrap_or_default())
}

/// Puts the held state a [`capture`] found back in place, once: when the
/// capture ends, or when its guard is dropped by an unwind.
struct Restore {
    outer: Option<Option<Vec<String>>>,
}

impl Restore {
    /// Restores the outer state and returns what this capture held.
    fn now(&mut self) -> Option<Vec<String>> {
        let outer = self.outer.take()?;
        HELD.with(|held| std::mem::replace(&mut *held.borrow_mut(), outer))
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        self.now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_holds_warnings_back_in_order() {
        let (value, held) = capture(|| {
            warn(format_args!("one"));
            warn(format_args!("two {}", 2));
            7
        });
        assert_eq!(value, 7);
        assert_eq!(held, ["one", "two 2"]);
    }

    #[test]
    fn a_nested_capture_keeps_its_own_and_hands_nothing_up() {
        let (_, outer) = capture(|| {
            warn(format_args!("outer"));
            let (_, inner) = capture(|| warn(format_args!("inner")));
            assert_eq!(inner, ["inner"]);
        });
        assert_eq!(outer, ["outer"]);
    }

    #[test]
    fn a_panic_inside_a_capture_leaves_the_outer_state_as_it_was() {
        let caught = std::panic::catch_unwind(|| {
            capture(|| -> () {
                warn(format_args!("lost"));
                panic!("inside the capture");
            })
        });
        assert!(caught.is_err());
        assert!(HELD.with(|held| held.borrow().is_none()));

        let (_, outer) = capture(|| {
            let _ = std::panic::catch_unwind(|| capture(|| -> () { panic!("inner") }));
            warn(format_args!("held"));
        });
        assert_eq!(outer, ["held"]);
    }

    #[test]
    fn nothing_is_held_outside_a_capture() {
        let (_, held) = capture(|| ());
        assert!(held.is_empty());
    }
}
