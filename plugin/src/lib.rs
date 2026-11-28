pub use rtk_engine::block::{Block, ContentFormat, Layer, Origin, Provenance, Subject};
pub use rtk_engine::method::{
    CostClass, Method, MethodCtx, MethodInfo, MethodKind, Outcome, Produces, Recovery,
};
pub use rtk_engine::registry::register;
pub use rtk_engine::task::{
    CorpusFile, TaskIntent, TaskKind, TaskOutcome, WorkerRequest, register_kind,
};

#[macro_export]
macro_rules! rtk_plugin {
    (methods: [$($method:expr),* $(,)?], kinds: [$($kind:expr),* $(,)?] $(,)?) => {
        pub fn __rtk_plugin_methods() -> ::std::vec::Vec<&'static dyn $crate::Method> {
            ::std::vec![
                $(
                    ::std::boxed::Box::leak(::std::boxed::Box::new($method))
                        as &'static dyn $crate::Method
                ),*
            ]
        }
        pub fn __rtk_plugin_kinds() -> ::std::vec::Vec<&'static dyn $crate::TaskKind> {
            ::std::vec![
                $(
                    ::std::boxed::Box::leak(::std::boxed::Box::new($kind))
                        as &'static dyn $crate::TaskKind
                ),*
            ]
        }
    };
    ($($method:expr),+ $(,)?) => {
        $crate::rtk_plugin!(methods: [$($method),+], kinds: []);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Uppercase;

    impl Method for Uppercase {
        fn name(&self) -> &'static str {
            "uppercase-demo"
        }
        fn info(&self) -> MethodInfo {
            MethodInfo::LOSSLESS_TRANSFORM
        }
        fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
            Outcome::Continue(block.text.to_uppercase())
        }
    }

    rtk_plugin!(Uppercase);

    struct Checklist;

    impl TaskKind for Checklist {
        fn name(&self) -> &'static str {
            "checklist-demo"
        }
        fn usage(&self) -> &'static str {
            "rtk <task> --question \"...\" <files>"
        }
        fn default_instructions(&self) -> &'static str {
            "Answer with a checklist."
        }
        fn assemble(
            &self,
            intent: &TaskIntent,
            corpus: &[CorpusFile],
            instructions: Option<&str>,
            _prompt: Option<&str>,
        ) -> Result<WorkerRequest, String> {
            let question = intent.question.as_deref().ok_or("needs --question")?;
            let files = corpus
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(WorkerRequest {
                system: instructions
                    .unwrap_or(self.default_instructions())
                    .to_string(),
                user: format!("{question}\n\n{files}"),
                max_tokens: 1024,
            })
        }
        fn handle(&self, output: &str) -> TaskOutcome {
            TaskOutcome::Print(output.trim().to_string())
        }
    }

    #[test]
    fn a_plugin_task_kind_registers_and_is_found_by_the_engine() {
        register_kind(&Checklist).expect("first registration succeeds");
        let kind = rtk_engine::task::find_kind("checklist-demo")
            .expect("the plugin kind must be registered");
        assert_eq!(kind.name(), "checklist-demo");
        assert!(
            rtk_engine::pipeline::task_kind_names().contains(&"checklist-demo"),
            "validation must accept tasks declaring the registered kind"
        );
        assert!(
            register_kind(&Checklist).is_err(),
            "duplicate names are rejected"
        );
        assert!(
            rtk_engine::task::register_kind(&rtk_engine::task::SUMMARIZE).is_err(),
            "built-in names cannot be shadowed"
        );
    }

    #[test]
    fn a_plugin_method_registers_and_runs_through_the_engine() {
        for m in __rtk_plugin_methods() {
            let _ = register(m);
        }
        let method = rtk_engine::registry::find("uppercase-demo")
            .expect("the plugin method must be registered");

        let origin = Origin::default();
        let prov = Provenance::default();
        let block = Block {
            text: "hello world",
            subject: Subject::Shell,
            format: ContentFormat::Term,
            origin: &origin,
            provenance: &prov,
        };
        let out = method.apply(&block, &MethodCtx::new("hello world"));
        assert_eq!(out, Outcome::Continue("HELLO WORLD".to_string()));
    }
}
