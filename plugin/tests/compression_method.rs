use rtk_plugin::{
    Block, ContentFormat, Method, MethodCtx, MethodInfo, Origin, Outcome, Provenance, Recovery,
    Subject, register, rtk_plugin,
};

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

fn block_of<'a>(text: &'a str, origin: &'a Origin, prov: &'a Provenance) -> Block<'a> {
    Block {
        text,
        subject: Subject::Shell,
        format: ContentFormat::Term,
        origin,
        provenance: prov,
    }
}

#[test]
fn a_user_compression_method_registers_and_reduces_bytes() {
    for m in __rtk_plugin_methods() {
        let _ = register(m);
    }
    let method = rtk_engine::registry::find("squeeze-blank")
        .expect("the plugin compression method must be registered");

    let raw = "build step 1   \n\n\n\ndone   \n\n\n\nbuild step 2\n\n\n\n\nok\n";
    let origin = Origin::default();
    let prov = Provenance::default();
    let out = match method.apply(&block_of(raw, &origin, &prov), &MethodCtx::new(raw)) {
        Outcome::Continue(text) => text,
        other => panic!("expected Continue, got {other:?}"),
    };

    assert!(
        out.len() < raw.len(),
        "compression must remove bytes: {} -> {}",
        raw.len(),
        out.len()
    );
    assert!(!out.contains("   \n"), "trailing whitespace must be gone");
    assert!(!out.contains("\n\n\n"), "blank runs must collapse to one");
    assert_eq!(
        out, "build step 1\n\ndone\n\nbuild step 2\n\nok\n",
        "exact squeezed form"
    );
}

#[test]
fn the_method_contract_is_visible_to_the_engine() {
    for m in __rtk_plugin_methods() {
        let _ = register(m);
    }
    let info =
        rtk_engine::registry::info_of("squeeze-blank").expect("MethodInfo must be queryable");
    assert!(info.lossy);
    assert_eq!(info.recovery, Recovery::Recall);
}
