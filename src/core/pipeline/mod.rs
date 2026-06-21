//! Generic filter layers applied around a command's own filter. A command picks
//! its `Routing` (which layers run); the pipeline applies the enabled layers in
//! either captured (`run`) or streaming (`stream`) mode, command filter last.

mod decorative;
mod dedup;
mod levels;

pub use levels::is_excluded;
pub use levels::set_group;
#[cfg(test)]
pub use levels::GROUPS;
pub use levels::{group_for_command, TruncateLevel};

/// The resolved truncate level, for `core::truncate::caps()`.
pub fn truncate_level() -> TruncateLevel {
    levels::current().truncate
}

pub fn route_toml_output(text: &str) -> String {
    Pipeline::with_routing(Routing {
        decorative: true,
        dedup: true,
    })
    .run(text, |s| s.to_string())
}

use crate::core::stream::StreamFilter;
use levels::Levels;

/// Per-command, code-level choice of which generic layers run around the
/// command's own filter. Not user-configurable; the custom filter always runs.
///
/// `dedup` defaults off. In `run` it executes after the custom filter
/// (post-parse), so it is safe per command. The streaming path still wraps it
/// pre-custom, so there it is only wired for the parser-less fallback.
#[derive(Clone, Copy, Debug)]
pub struct Routing {
    pub decorative: bool,
    pub dedup: bool,
}

impl Default for Routing {
    fn default() -> Self {
        Self {
            decorative: true,
            dedup: false,
        }
    }
}

pub struct Pipeline {
    routing: Routing,
    levels: Levels,
}

impl Pipeline {
    pub fn with_routing(routing: Routing) -> Self {
        Self {
            routing,
            levels: *levels::current(),
        }
    }

    // no filter enabled → native exec
    pub fn is_noop(&self) -> bool {
        let dec =
            self.routing.decorative && self.levels.decorative != decorative::DecorativeLevel::None;
        let ddp = self.routing.dedup && self.levels.dedup != dedup::DedupLevel::None;
        !dec && !ddp
    }

    pub fn run(&self, raw: &str, custom: impl Fn(&str) -> String) -> String {
        let mut data = raw.to_string();
        if self.routing.decorative {
            data = decorative::apply(&data, self.levels.decorative);
        }
        // dedup runs on the filter's OUTPUT (post-parse) so it can't corrupt a
        // parser; gated by the user level (off by default), so it works per-group.
        let mut out = custom(&data);
        if self.levels.dedup != dedup::DedupLevel::None {
            out = dedup::apply(&out, self.levels.dedup);
        }
        out
    }

    pub fn stream<'a>(&self, inner: Box<dyn StreamFilter + 'a>) -> Box<dyn StreamFilter + 'a> {
        let mut filter = inner;
        if self.routing.dedup {
            filter = dedup::wrap_stream(filter, self.levels.dedup);
        }
        if self.routing.decorative {
            filter = decorative::wrap_stream(filter, self.levels.decorative);
        }
        filter
    }

    #[cfg(test)]
    fn with_levels(routing: Routing, levels: Levels) -> Self {
        Self { routing, levels }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use decorative::DecorativeLevel as DL;
    use dedup::DedupLevel as DD;

    // Pin levels so tests don't depend on the machine's env/config.
    fn lv(decorative: DL, dedup: DD) -> Levels {
        Levels {
            decorative,
            dedup,
            truncate: TruncateLevel::default(),
        }
    }

    #[test]
    fn toml_output_is_polished_not_corrupted() {
        let routing = Routing {
            decorative: true,
            dedup: false,
        };
        let toml_out = "\x1b[32mok\x1b[0m   \n\n\nrest";
        let cleaned = Pipeline::with_levels(routing, lv(DL::Reasonable, DD::None))
            .run(toml_out, |s| s.to_string());
        assert_eq!(cleaned, "ok\n\nrest");
        let raw =
            Pipeline::with_levels(routing, lv(DL::None, DD::None)).run(toml_out, |s| s.to_string());
        assert_eq!(raw, toml_out);
    }

    struct Echo;
    impl StreamFilter for Echo {
        fn feed_line(&mut self, line: &str) -> Option<String> {
            Some(line.to_string())
        }
        fn flush(&mut self) -> String {
            String::new()
        }
    }

    #[test]
    fn run_applies_routing_then_custom() {
        let out = Pipeline::with_levels(Routing::default(), lv(DL::Reasonable, DD::None))
            .run("\x1b[32mx\x1b[0m\ny", |s| {
                format!("[{}]", s.replace('\n', "|"))
            });
        assert_eq!(out, "[x|y]");
    }

    #[test]
    fn run_without_routing_passes_raw_to_custom() {
        let raw = "\x1b[32mx\x1b[0m";
        let off = Routing {
            decorative: false,
            dedup: false,
        };
        let out =
            Pipeline::with_levels(off, lv(DL::Reasonable, DD::Exact)).run(raw, |s| s.to_string());
        assert_eq!(out, raw);
    }

    #[test]
    fn run_with_dedup_collapses_repeats() {
        let routing = Routing {
            decorative: false,
            dedup: true,
        };
        let out = Pipeline::with_levels(routing, lv(DL::Reasonable, DD::Exact))
            .run("a\na\nb", |s| s.to_string());
        assert_eq!(out, "[×2] a\nb");
    }

    #[test]
    fn decorative_runs_before_dedup() {
        // Lines identical only after ANSI strip must collapse — proves order.
        let routing = Routing {
            decorative: true,
            dedup: true,
        };
        let out = Pipeline::with_levels(routing, lv(DL::Reasonable, DD::Exact))
            .run("\x1b[31mERR\x1b[0m\n\x1b[32mERR\x1b[0m", |s| s.to_string());
        assert_eq!(out, "[×2] ERR");
    }

    #[test]
    fn stream_decorative_then_dedup() {
        let routing = Routing {
            decorative: true,
            dedup: true,
        };
        let mut f =
            Pipeline::with_levels(routing, lv(DL::Reasonable, DD::Exact)).stream(Box::new(Echo));
        assert_eq!(f.feed_line("\x1b[31mERR\x1b[0m"), None);
        assert_eq!(f.feed_line("\x1b[32mERR\x1b[0m"), None);
        assert_eq!(f.flush(), "[×2] ERR");
    }

    #[test]
    fn dedup_only_leaves_ansi_for_custom() {
        let routing = Routing {
            decorative: false,
            dedup: true,
        };
        let out = Pipeline::with_levels(routing, lv(DL::Reasonable, DD::Exact))
            .run("\x1b[31mx\x1b[0m\n\x1b[31mx\x1b[0m", |s| s.to_string());
        assert_eq!(out, "[×2] \x1b[31mx\x1b[0m");
    }

    #[test]
    fn stream_decorates_lines_before_inner() {
        let mut f = Pipeline::with_levels(Routing::default(), lv(DL::Reasonable, DD::None))
            .stream(Box::new(Echo));
        let out = f.feed_line("\x1b[32mok\x1b[0m").unwrap();
        assert!(!out.contains('\x1b') && out.contains("ok"));
    }

    #[test]
    fn stream_without_routing_is_passthrough() {
        let off = Routing {
            decorative: false,
            dedup: false,
        };
        let mut f =
            Pipeline::with_levels(off, lv(DL::Reasonable, DD::Exact)).stream(Box::new(Echo));
        assert_eq!(
            f.feed_line("\x1b[32mok\x1b[0m"),
            Some("\x1b[32mok\x1b[0m".to_string())
        );
    }
}
