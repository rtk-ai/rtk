use rtk_engine::pipeline::{
    Code, Diagnostic, PipelineFile, Scope, SourceFile, ValidateCtx, builtin, has_errors, parse,
    resolve,
};

const GUIDE: &[(&str, &str)] = &[
    (
        "overview.md",
        include_str!("../../docs/guide/pipelines/overview.md"),
    ),
    (
        "writing-pipelines.md",
        include_str!("../../docs/guide/pipelines/writing-pipelines.md"),
    ),
    (
        "conditions.md",
        include_str!("../../docs/guide/pipelines/conditions.md"),
    ),
    (
        "methods.md",
        include_str!("../../docs/guide/pipelines/methods.md"),
    ),
    (
        "directives.md",
        include_str!("../../docs/guide/pipelines/directives.md"),
    ),
    (
        "tasks.md",
        include_str!("../../docs/guide/pipelines/tasks.md"),
    ),
    (
        "reference-pipelines.md",
        include_str!("../../docs/guide/pipelines/reference-pipelines.md"),
    ),
    (
        "commands.md",
        include_str!("../../docs/guide/pipelines/commands.md"),
    ),
    (
        "proxy/compression.md",
        include_str!("../../docs/guide/proxy/compression.md"),
    ),
];

const CONTEXT: &str = r#"
schema_version = 1

[pipeline]
extends = ["rtk:posthook", "rtk:proxy", "rtk:delegate-reads"]

[method.gentle]
chain = ["ansi", "dedup"]

[upstream.flash]
provider    = "google"
model       = "gemini-2.5-flash"
api_key_env = "GEMINI_API_KEY"

[task.bulk-read]
upstream = "flash"

[task.code-write]
upstream = "flash"

[upstream.judge]
provider    = "judge"
model       = "judge-v1"
base_url    = "https://judge.example.com/v1/systemone"
api_key_env = "JUDGE_API_KEY"

[task.digest-failures]
kind     = "digest"
upstream = "flash"
"#;

fn toml_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (index, line) in markdown.lines().enumerate() {
        match &mut current {
            None if line.trim_start() == "```toml" => {
                current = Some((index + 1, String::new()));
            }
            Some((start, body)) => {
                if line.trim_start().starts_with("```") {
                    blocks.push((*start, std::mem::take(body)));
                    current = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            None => {}
        }
    }
    blocks
}

fn validate_block(text: &str, with_context: bool) -> Option<Vec<Diagnostic>> {
    let parsed = parse(text, "guide-block");
    let file = parsed.file?;
    if parsed.diagnostics.iter().any(|d| d.code == Code::Load) {
        return None;
    }
    let mut diagnostics = parsed.diagnostics;
    let mut sources = Vec::new();
    if with_context {
        let context = parse(CONTEXT, "context");
        sources.push(SourceFile {
            scope: Scope::User,
            label: "context".to_string(),
            file: context.file.expect("context parses"),
        });
    }
    sources.push(SourceFile {
        scope: Scope::Project,
        label: "guide-block".to_string(),
        file,
    });
    let (resolved, resolve_diags) = resolve(&sources);
    diagnostics.extend(resolve_diags);
    diagnostics.extend(rtk_engine::pipeline::validate(
        &resolved,
        &ValidateCtx::default(),
    ));
    Some(diagnostics)
}

/// `Some(name)` when the block reproduces a compiled pipeline.
fn declared_builtin(block: &str) -> Option<String> {
    let file = parse(block, "probe").file?;
    let name = file.name?;
    builtin(&name).map(|_| name)
}

/// The parts of a pipeline a reader relies on: what it inherits, which rules sit
/// where, which directives it adds, and the guards. Deliberately not a full
/// equality check — comments and key order are free to differ between a guide
/// block and the file it documents.
fn summarize(file: &PipelineFile) -> String {
    let mut out = String::new();
    out.push_str(&format!("extends={:?}\n", file.extends));
    for (position, rules) in &file.rules {
        let names: Vec<&str> = rules
            .iter()
            .map(|r| r.name.as_deref().unwrap_or("?"))
            .collect();
        out.push_str(&format!("{}: {:?}\n", position.render(), names));
    }
    let mut directives: Vec<String> = file
        .directives
        .iter()
        .map(|d| {
            d.use_
                .clone()
                .or_else(|| d.name.clone())
                .unwrap_or_default()
        })
        .collect();
    directives.sort();
    out.push_str(&format!("directives={directives:?}\n"));
    let mut overrides: Vec<&String> = file.overrides.keys().collect();
    overrides.sort();
    out.push_str(&format!("overrides={overrides:?}\n"));
    out.push_str(&format!(
        "tasks={:?}\n",
        file.tasks.keys().collect::<Vec<_>>()
    ));
    out.push_str(&format!(
        "guards: never_worse={:?} min_size={:?}\n",
        file.guards.never_worse, file.guards.min_size
    ));
    out
}

#[test]
fn every_valid_toml_block_in_the_guide_validates() {
    let blocks: Vec<(&str, usize, String)> = GUIDE
        .iter()
        .flat_map(|(page, text)| {
            toml_blocks(text)
                .into_iter()
                .map(move |(line, block)| (*page, line, block))
        })
        .collect();
    assert!(
        blocks.len() >= 10,
        "the guide should carry its examples ({} found)",
        blocks.len()
    );
    let mut checked = 0;
    for (page, line, block) in &blocks {
        if block.contains("filters.toml") || block.contains("[filters.") {
            continue;
        }
        // A block that does not parse is the failure this test exists to catch:
        // a reader copying it gets a parse error and a silent fallback. Skipping
        // it would make the guard blind to exactly that, and the `>= 10` floor
        // above would pass on whatever survived.
        if let Err(e) = toml::from_str::<toml::Table>(block) {
            panic!(
                "guide block at {page}:{line} is not valid TOML: {e}\n\
                 a reader copying this gets a parse error and falls back to \
                 rtk:default with no explanation\n---\n{block}\n---"
            );
        }

        // A block that names a built-in is a reproduction of compiled content,
        // not a file a user would write: validating it as one trips E012 on the
        // rules it necessarily redeclares. What matters is that it still says
        // what ships, so compare it to the built-in instead.
        if let Some(name) = declared_builtin(block) {
            let shipped = builtin(&name)
                .unwrap_or_else(|| panic!("guide block at {page}:{line} names unknown {name}"));
            let documented = parse(block, "guide-block")
                .file
                .unwrap_or_else(|| panic!("guide block at {page}:{line} did not parse"));
            assert_eq!(
                summarize(&documented),
                summarize(shipped),
                "guide block at {page}:{line} no longer describes {name} as it ships \
                 — engine/builtin/ is the source of truth"
            );
            checked += 1;
            continue;
        }
        let standalone = validate_block(block, false);
        let result = match &standalone {
            Some(diags) if !has_errors(diags) => standalone.clone(),
            _ => validate_block(block, true),
        };
        let Some(diagnostics) = result else {
            continue;
        };
        let errors: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.severity() == rtk_engine::pipeline::Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "guide block at {page}:{line} fails validation:\n{block}\n{errors:#?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "too few guide blocks were checkable ({checked}) — extraction broke?"
    );
}
