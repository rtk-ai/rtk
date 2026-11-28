use rtk_plugin::{Block, Method, MethodCtx, MethodInfo, Outcome, Recovery, register, rtk_plugin};

struct SqueezeBlank;

impl Method for SqueezeBlank {
    fn name(&self) -> &'static str {
        "squeeze-blank"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo {
            lossy: true,
            recovery: Recovery::Recall,
            ..MethodInfo::LOSSLESS_TRANSFORM
        }
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        let mut out = String::with_capacity(block.text.len());
        let mut pending_blanks = 0usize;
        for line in block.text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                pending_blanks += 1;
                continue;
            }
            if pending_blanks > 0 {
                out.push('\n');
                pending_blanks = 0;
            }
            out.push_str(trimmed);
            out.push('\n');
        }
        Outcome::Continue(out)
    }
}

rtk_plugin!(SqueezeBlank);

pub fn register_all() {
    for method in __rtk_plugin_methods() {
        let _ = register(method);
    }
}
