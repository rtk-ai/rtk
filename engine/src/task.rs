#[derive(Debug, Clone)]
pub struct WorkerRequest {
    pub system: String,
    pub user: String,
    pub max_tokens: usize,
}

pub enum TaskOutcome {
    Print(String),
    WriteFile(String),
}

#[derive(Debug, Clone, Default)]
pub struct TaskIntent {
    pub question: Option<String>,
    pub spec: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CorpusFile {
    pub path: String,
    pub content: String,
}

pub trait TaskKind: Sync {
    fn name(&self) -> &'static str;
    fn usage(&self) -> &'static str;
    fn default_instructions(&self) -> &'static str;
    fn assemble(
        &self,
        intent: &TaskIntent,
        corpus: &[CorpusFile],
        instructions: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<WorkerRequest, String>;
    fn handle(&self, output: &str) -> TaskOutcome;
}

fn render_corpus(corpus: &[CorpusFile]) -> String {
    corpus
        .iter()
        .map(|f| format!("=== {} ===\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn fill_template(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in pairs {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

pub struct Summarize;

// Default worker instructions for the bulk-read interface.
const SUMMARIZE_INSTRUCTIONS: &str = "\
You answer one question about the attached files for another engineer's \
coding agent. Reply with terse bullets only: the facts that answer the \
question, exact identifiers, signatures, paths and line-relevant details, \
nothing else. Never summarise style or restate the question. If the answer \
is not in the files, say so in one bullet.";

const SUMMARIZE_TEMPLATE: &str = "Question: ${question}\n\nFiles:\n\n${files}";

impl TaskKind for Summarize {
    fn name(&self) -> &'static str {
        "summarize"
    }

    fn usage(&self) -> &'static str {
        "rtk <task> --question \"...\" <paths...>"
    }

    fn default_instructions(&self) -> &'static str {
        SUMMARIZE_INSTRUCTIONS
    }

    fn assemble(
        &self,
        intent: &TaskIntent,
        corpus: &[CorpusFile],
        instructions: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<WorkerRequest, String> {
        let question = intent
            .question
            .as_deref()
            .ok_or("summarize needs --question \"<what you need to know>\"")?;
        if corpus.is_empty() {
            return Err("summarize needs at least one file path".to_string());
        }
        let user = fill_template(
            prompt.unwrap_or(SUMMARIZE_TEMPLATE),
            &[("question", question), ("files", &render_corpus(corpus))],
        );
        Ok(WorkerRequest {
            system: instructions.unwrap_or(SUMMARIZE_INSTRUCTIONS).to_string(),
            user,
            max_tokens: 2048,
        })
    }

    fn handle(&self, output: &str) -> TaskOutcome {
        TaskOutcome::Print(output.trim().to_string())
    }
}

pub struct Generate;

// Default worker instructions for the code-write interface.
const GENERATE_INSTRUCTIONS: &str = "\
You write one complete file from a spec, matching the conventions of the \
attached reference file: same style, imports, naming, error handling and \
test idioms. Output ONLY the file's content — no fences, no commentary, \
no explanation before or after.";

const GENERATE_TEMPLATE: &str =
    "Spec: ${spec}\n\nTarget path: ${target}\n\nReference file:\n\n${reference}";

impl TaskKind for Generate {
    fn name(&self) -> &'static str {
        "generate"
    }

    fn usage(&self) -> &'static str {
        "rtk <task> --spec \"...\" --reference <file> --target <file>"
    }

    fn default_instructions(&self) -> &'static str {
        GENERATE_INSTRUCTIONS
    }

    fn assemble(
        &self,
        intent: &TaskIntent,
        corpus: &[CorpusFile],
        instructions: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<WorkerRequest, String> {
        let spec = intent
            .spec
            .as_deref()
            .ok_or("generate needs --spec \"...\"")?;
        let target = intent
            .target
            .as_deref()
            .ok_or("generate needs --target <file>")?;
        let reference = corpus.first().ok_or(
            "generate refuses to run without --reference <file>: without a file to match, \
             the worker writes context-free code",
        )?;
        let user = fill_template(
            prompt.unwrap_or(GENERATE_TEMPLATE),
            &[
                ("spec", spec),
                ("target", target),
                ("reference", &render_corpus(std::slice::from_ref(reference))),
            ],
        );
        Ok(WorkerRequest {
            system: instructions.unwrap_or(GENERATE_INSTRUCTIONS).to_string(),
            user,
            max_tokens: 8192,
        })
    }

    fn handle(&self, output: &str) -> TaskOutcome {
        let trimmed = output.trim();
        let body = trimmed
            .strip_prefix("```")
            .map(|rest| {
                let rest = rest.split_once('\n').map(|(_, b)| b).unwrap_or(rest);
                rest.strip_suffix("```").unwrap_or(rest)
            })
            .unwrap_or(trimmed);
        TaskOutcome::WriteFile(format!("{}\n", body.trim_end()))
    }
}

pub struct Digest;

const DIGEST_INSTRUCTIONS: &str = "\
You answer one question about the output of a command or tool for another \
engineer's coding agent. Reply with terse bullets only: what happened, the \
exact failures, identifiers, paths and line numbers that matter, and nothing \
else. Quote error text verbatim rather than paraphrasing it. Never restate \
the question, never describe the output's formatting, never add advice that \
was not asked for. If the output does not answer the question, say so in one \
bullet.";

const DIGEST_TEMPLATE: &str = "Question: ${question}\n\nOutput:\n\n${files}";

const DIGEST_DEFAULT_QUESTION: &str =
    "What happened, and what would an engineer need to act on it?";

impl TaskKind for Digest {
    fn name(&self) -> &'static str {
        "digest"
    }

    fn usage(&self) -> &'static str {
        "rtk <task> --question \"...\" -- <command>"
    }

    fn default_instructions(&self) -> &'static str {
        DIGEST_INSTRUCTIONS
    }

    fn assemble(
        &self,
        intent: &TaskIntent,
        corpus: &[CorpusFile],
        instructions: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<WorkerRequest, String> {
        if corpus.is_empty() {
            return Err(
                "digest needs output to read: a command after `--`, or a file path".to_string(),
            );
        }
        let question = intent
            .question
            .as_deref()
            .unwrap_or(DIGEST_DEFAULT_QUESTION);
        let user = fill_template(
            prompt.unwrap_or(DIGEST_TEMPLATE),
            &[("question", question), ("files", &render_corpus(corpus))],
        );
        Ok(WorkerRequest {
            system: instructions.unwrap_or(DIGEST_INSTRUCTIONS).to_string(),
            user,
            max_tokens: 2048,
        })
    }

    fn handle(&self, output: &str) -> TaskOutcome {
        TaskOutcome::Print(output.trim().to_string())
    }
}

pub static SUMMARIZE: Summarize = Summarize;
pub static GENERATE: Generate = Generate;
pub static DIGEST: Digest = Digest;

pub static TASK_KINDS: &[&(dyn TaskKind + 'static)] = &[&SUMMARIZE, &GENERATE, &DIGEST];

fn registered_kinds() -> &'static std::sync::Mutex<Vec<&'static (dyn TaskKind + 'static)>> {
    static REGISTERED: std::sync::OnceLock<
        std::sync::Mutex<Vec<&'static (dyn TaskKind + 'static)>>,
    > = std::sync::OnceLock::new();
    REGISTERED.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub fn register_kind(kind: &'static (dyn TaskKind + 'static)) -> Result<(), String> {
    let name = kind.name();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Err(format!(
            "task kind name `{name}` must be alphanumeric with - or _, starting with a letter"
        ));
    }
    if find_kind(name).is_some() {
        return Err(format!("task kind `{name}` is already registered"));
    }
    registered_kinds()
        .lock()
        .map_err(|_| "kind registry poisoned".to_string())?
        .push(kind);
    Ok(())
}

pub fn kinds() -> Vec<&'static dyn TaskKind> {
    let mut out: Vec<&'static dyn TaskKind> = TASK_KINDS
        .iter()
        .copied()
        .map(|k| k as &'static dyn TaskKind)
        .collect();
    if let Ok(reg) = registered_kinds().lock() {
        out.extend(reg.iter().copied().map(|k| k as &'static dyn TaskKind));
    }
    out
}

pub fn find_kind(name: &str) -> Option<&'static dyn TaskKind> {
    kinds().into_iter().find(|k| k.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<CorpusFile> {
        vec![CorpusFile {
            path: "src/service.rs".to_string(),
            content: "pub fn serve() {}".to_string(),
        }]
    }

    #[test]
    fn summarize_assembles_question_and_files() {
        let request = SUMMARIZE
            .assemble(
                &TaskIntent {
                    question: Some("what does serve do?".to_string()),
                    ..TaskIntent::default()
                },
                &corpus(),
                None,
                None,
            )
            .expect("assembles");
        assert!(request.user.contains("what does serve do?"));
        assert!(request.user.contains("=== src/service.rs ==="));
        assert!(request.system.contains("terse bullets"));
    }

    #[test]
    fn summarize_without_question_refuses() {
        assert!(
            SUMMARIZE
                .assemble(&TaskIntent::default(), &corpus(), None, None)
                .is_err()
        );
    }

    #[test]
    fn generate_refuses_without_reference() {
        let err = GENERATE
            .assemble(
                &TaskIntent {
                    spec: Some("a service".to_string()),
                    target: Some("src/new.rs".to_string()),
                    ..TaskIntent::default()
                },
                &[],
                None,
                None,
            )
            .unwrap_err();
        assert!(err.contains("--reference"));
    }

    #[test]
    fn generate_strips_code_fences_from_output() {
        let TaskOutcome::WriteFile(content) = GENERATE.handle("```rust\nfn a() {}\n```") else {
            panic!("generate writes a file");
        };
        assert_eq!(content, "fn a() {}\n");
    }

    #[test]
    fn custom_instructions_and_prompt_override_the_defaults() {
        let request = SUMMARIZE
            .assemble(
                &TaskIntent {
                    question: Some("q".to_string()),
                    ..TaskIntent::default()
                },
                &corpus(),
                Some("be a reviewer"),
                Some("Q=${question} F=${files}"),
            )
            .expect("assembles");
        assert_eq!(request.system, "be a reviewer");
        assert!(request.user.starts_with("Q=q F=="));
    }

    #[test]
    fn kinds_are_registered_by_name() {
        assert!(find_kind("summarize").is_some());
        assert!(find_kind("generate").is_some());
        assert!(find_kind("digest").is_some());
        assert!(find_kind("judge").is_none());
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    fn output() -> Vec<CorpusFile> {
        vec![CorpusFile {
            path: "npm test".to_string(),
            content: "FAIL src/a.test.ts\n  expected 3 got 4".to_string(),
        }]
    }

    #[test]
    fn digest_assembles_question_and_output() {
        let request = DIGEST
            .assemble(
                &TaskIntent {
                    question: Some("did it pass?".to_string()),
                    ..TaskIntent::default()
                },
                &output(),
                None,
                None,
            )
            .expect("assembles");
        assert!(request.user.contains("did it pass?"));
        assert!(request.user.contains("=== npm test ==="));
        assert!(request.user.contains("expected 3 got 4"));
    }

    #[test]
    fn digest_falls_back_to_a_default_question() {
        let request = DIGEST
            .assemble(&TaskIntent::default(), &output(), None, None)
            .expect("assembles without --question");
        assert!(request.user.contains(DIGEST_DEFAULT_QUESTION));
    }

    #[test]
    fn digest_without_output_refuses() {
        let err = DIGEST
            .assemble(&TaskIntent::default(), &[], None, None)
            .unwrap_err();
        assert!(err.contains("needs output"));
    }
}
