use crate::block::{Layer, Subject};
use crate::method::{CostClass, Method, MethodInfo, Recovery};
use std::sync::{Mutex, OnceLock};

pub mod names {
    pub const ANSI: &str = "ansi";
    pub const TOON: &str = "toon";
    pub const MINIFY_JSON: &str = "minify-json";
    pub const WEB_MD: &str = "web-md";
    pub const TRUNCATE: &str = "truncate";
    pub const GREP_GROUP: &str = "grep-group";
    pub const BASE64_ELIDE: &str = "base64-elide";
    pub const LOCKFILE: &str = "lockfile";
    pub const MD_SLIM: &str = "md-slim";
    pub const PROGRESS: &str = "progress";
    pub const DEDUP: &str = "dedup";
    pub const CAP: &str = "cap";
    pub const STRIP_LINE_NUMBERS: &str = "strip-line-numbers";
    pub const CCR: &str = "ccr";
    pub const CACHE_ALIGN: &str = "cache-align";
    pub const SHAPE: &str = "shape";
    pub const RLE: &str = "rle";
    pub const RETIRE_POINTER: &str = "retire-pointer";
    pub const RTK_AWARENESS: &str = "rtk-awareness";
    pub const TERSE_OUTPUT: &str = "terse-output";
    pub const ROUTE_TASKS: &str = "route-tasks";
    pub const RESERVED: &[&str] = &[
        "unicode",
        "ipynb-strip",
        "tree-sitter",
        "pixel",
        "classify",
        "route",
        "terse-prose",
        "compact-tools",
        "redact",
    ];
}

pub static METHODS: &[&(dyn Method + 'static)] = &[
    &crate::methods::ANSI,
    &crate::methods::TOON,
    &crate::methods::MINIFY_JSON,
    &crate::methods::WEB_MD,
    &crate::methods::TRUNCATE,
    &crate::methods::GREP_GROUP,
    &crate::methods::BASE64_ELIDE,
    &crate::methods::LOCKFILE,
    &crate::methods::MD_SLIM,
    &crate::methods::PROGRESS,
    &crate::methods::DEDUP,
    &crate::methods::CAP,
    &crate::methods::STRIP_LINE_NUMBERS,
    &crate::methods::SHAPE,
    &crate::methods::RLE,
];

fn registered() -> &'static Mutex<Vec<&'static (dyn Method + 'static)>> {
    static REGISTERED: OnceLock<Mutex<Vec<&'static (dyn Method + 'static)>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register(method: &'static (dyn Method + 'static)) -> Result<(), String> {
    let name = method.name();
    if find(name).is_some() {
        return Err(format!("method `{name}` is already registered"));
    }
    registered()
        .lock()
        .map_err(|_| "registry poisoned".to_string())?
        .push(method);
    Ok(())
}

pub fn find(name: &str) -> Option<&'static dyn Method> {
    if let Some(method) = METHODS.iter().find(|m| m.name() == name).copied() {
        return Some(method);
    }
    registered()
        .lock()
        .ok()?
        .iter()
        .find(|m| m.name() == name)
        .copied()
        .map(|m| m as &'static dyn Method)
}

/// Terminal slot reserved for secret redaction (`redact`): a method
/// registered here runs last on every hook-site emission, before any content
/// is persisted (recall files, caches), and is exempt from `never_worse`.
/// OSS registers nothing here.
pub fn register_redact(method: &'static (dyn Method + 'static)) -> Result<(), String> {
    redact_slot()
        .set(method)
        .map_err(|_| "redact slot already filled".to_string())
}

fn redact_slot() -> &'static OnceLock<&'static (dyn Method + 'static)> {
    static SLOT: OnceLock<&'static (dyn Method + 'static)> = OnceLock::new();
    &SLOT
}

pub fn redact() -> Option<&'static dyn Method> {
    redact_slot()
        .get()
        .copied()
        .map(|m| m as &'static dyn Method)
}

pub fn chain(name: &str) -> Option<&'static [&'static dyn Method]> {
    crate::methods::named_chain(name)
}

pub const CHAIN_NAMES: &[&str] = &["term", "json", "web", "matches"];

pub fn chain_info(methods: &'static [&'static dyn Method]) -> MethodInfo {
    let mut info = MethodInfo::LOSSLESS_TRANSFORM;
    for method in methods {
        let m = method.info();
        if m.lossy {
            info.lossy = true;
            if info.recovery == Recovery::None {
                info.recovery = m.recovery;
            }
        }
        if m.cost > info.cost {
            info.cost = m.cost;
        }
        if !m.cache_stable {
            info.cache_stable = false;
        }
        if !m.idempotent {
            info.idempotent = false;
        }
    }
    // The type-checking trio: a chain consumes what its first member
    // consumes, produces what its last member produces, and is terminal when
    // its last member is. Without these every named chain would default to
    // "consumes anything, produces Same, never terminal" — which would let
    // `[method.mine] chain = ["toon"]` smuggle a terminal method past E008.
    if let Some(first) = methods.first() {
        info.consumes = first.info().consumes;
    }
    if let Some(last) = methods.last() {
        let m = last.info();
        info.produces = m.produces;
        info.terminal = m.terminal;
    }
    info
}

pub static PROXY_METHOD_INFO: &[(&str, MethodInfo)] = &[
    (
        names::CCR,
        MethodInfo {
            lossy: true,
            recovery: Recovery::RetrieveTool,
            idempotent: false,
            cost: CostClass::Slow,
            saves_on: &[Layer::History, Layer::ToolResult],
            ..MethodInfo::LOSSLESS_TRANSFORM
        },
    ),
    (
        names::CACHE_ALIGN,
        MethodInfo {
            saves_on: &[Layer::Cache],
            ..MethodInfo::LOSSLESS_TRANSFORM
        },
    ),
    (
        names::RETIRE_POINTER,
        MethodInfo {
            lossy: true,
            recovery: Recovery::Pointer,
            idempotent: false,
            cost: CostClass::Fast,
            saves_on: &[Layer::Write, Layer::History],
            ..MethodInfo::LOSSLESS_TRANSFORM
        },
    ),
];

pub fn proxy_info(name: &str) -> Option<MethodInfo> {
    PROXY_METHOD_INFO
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, info)| *info)
}

pub fn info_of(name: &str) -> Option<MethodInfo> {
    if let Some(method) = find(name) {
        return Some(method.info());
    }
    proxy_info(name)
}

pub const PROXY_NORMALIZE_METHODS: &[&str] = &["ansi", "progress", "dedup", "strip-line-numbers"];

pub fn safe_without_recall(method: &dyn Method) -> bool {
    let info = method.info();
    info.cache_stable && info.deterministic && info.recovery != Recovery::Recall
}

pub fn proxy_skip_reason(name: &str, part: Subject) -> Option<&'static str> {
    let only_at = |home: Subject, reason: &'static str| (part != home).then_some(reason);
    match name {
        names::CCR => only_at(
            Subject::ToolResult,
            "ccr runs only at proxy.request.tool_result",
        ),
        names::RETIRE_POINTER => only_at(
            Subject::ToolUse,
            "retire-pointer runs only at proxy.request.tool_use",
        ),
        names::CACHE_ALIGN => only_at(
            Subject::Cache,
            "cache-align runs only at proxy.request.cache",
        ),
        n if PROXY_NORMALIZE_METHODS.contains(&n) => only_at(
            Subject::ToolResult,
            "the proxy normalises only at proxy.request.tool_result",
        ),
        n => {
            let method = find(n)?;
            if !matches!(part, Subject::ToolResult | Subject::Any) {
                return Some("the proxy runs engine methods only at proxy.request.tool_result");
            }
            (!safe_without_recall(method)).then_some(
                "it keeps the original only in a recall copy, which the proxy cannot store — use it under hook.after_tool",
            )
        }
    }
}

pub fn is_proxy_resident(name: &str) -> bool {
    proxy_info(name).is_some()
}

pub fn known_names() -> Vec<String> {
    let mut out: Vec<String> = METHODS.iter().map(|m| m.name().to_string()).collect();
    out.extend(CHAIN_NAMES.iter().map(|n| n.to_string()));
    out.extend(PROXY_METHOD_INFO.iter().map(|(n, _)| n.to_string()));
    if let Ok(extra) = registered().lock() {
        out.extend(extra.iter().map(|m| m.name().to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_method_is_not_found() {
        assert!(find("no-such-method").is_none());
    }

    #[test]
    fn chains_resolve_to_the_shipped_composites() {
        let term: Vec<&str> = chain("term").unwrap().iter().map(|m| m.name()).collect();
        assert_eq!(
            term,
            vec!["ansi", "progress", "dedup", "base64-elide", "cap"]
        );
        let json: Vec<&str> = chain("json").unwrap().iter().map(|m| m.name()).collect();
        assert_eq!(json, vec!["base64-elide", "toon"]);
        let web: Vec<&str> = chain("web").unwrap().iter().map(|m| m.name()).collect();
        assert_eq!(
            web,
            vec!["ansi", "base64-elide", "web-md", "toon", "md-slim"]
        );
        let matches: Vec<&str> = chain("matches").unwrap().iter().map(|m| m.name()).collect();
        assert_eq!(matches, vec!["grep-group"]);
        assert!(chain("gentle").is_none());
    }

    #[test]
    fn chain_info_aggregates_lossiness_and_cost() {
        let term = chain_info(chain("term").unwrap());
        assert!(term.lossy, "cap makes term lossy");
        assert_eq!(term.recovery, Recovery::Recall);
        let matches = chain_info(chain("matches").unwrap());
        // grep-group trims indentation, so the matches chain is lossy too;
        // recall carries the original.
        assert!(matches.lossy);
        assert_eq!(matches.recovery, Recovery::Recall);
    }

    #[test]
    fn redact_slot_is_empty_in_oss() {
        assert!(redact().is_none());
    }

    #[test]
    fn reserved_names_do_not_collide_with_known_names() {
        let known = known_names();
        for reserved in names::RESERVED {
            assert!(!known.contains(&reserved.to_string()), "{reserved}");
        }
    }

    #[test]
    fn every_static_method_name_is_unique() {
        let names = known_names();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len());
    }
}
