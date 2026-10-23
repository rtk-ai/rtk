use super::*;

pub(super) struct OnrampSpec {
    pub label: &'static str,
    pub config: &'static str,
    pub key: &'static str,
    pub path: &'static str,
    pub manual_option: &'static str,
    pub autostart_option: Option<&'static str>,
    pub autostart_done: &'static str,
    pub autostart_persistence: &'static str,
}

pub(super) trait ProxyOnramp {
    fn spec(&self) -> &'static OnrampSpec;
    fn path(&self) -> &'static str {
        self.spec().path
    }
    fn current_target(&self) -> Result<Option<String>>;
    fn autostart_present(&self) -> Result<bool>;
    fn manual_line(&self, base_url: &str) -> String;
    fn write(&self, base_url: &str, autostart: bool) -> Result<Vec<String>>;
}

pub(super) const CLAUDE_SPEC: OnrampSpec = OnrampSpec {
    label: "Claude Code",
    config: "settings.json",
    key: "env.ANTHROPIC_BASE_URL",
    path: "",
    manual_option: "Neither — print the export line; settings.json is not touched",
    autostart_option: Some(
        "SessionStart hook — start it automatically when a Claude session begins (recommended)",
    ),
    autostart_done: "settings.json: SessionStart hook registered — the proxy starts with each session",
    autostart_persistence: "SessionStart autostart",
};

pub(super) struct ClaudeOnramp {
    pub settings_path: PathBuf,
}

impl ClaudeOnramp {
    fn root(&self) -> Result<serde_json::Value> {
        if !self.settings_path.exists() {
            return Ok(serde_json::json!({}));
        }
        let content = fs::read_to_string(&self.settings_path)?;
        let content = strip_leading_bom(&content);
        if content.trim().is_empty() {
            Ok(serde_json::json!({}))
        } else {
            Ok(from_json_str(content)?)
        }
    }
}

impl ProxyOnramp for ClaudeOnramp {
    fn spec(&self) -> &'static OnrampSpec {
        &CLAUDE_SPEC
    }

    fn current_target(&self) -> Result<Option<String>> {
        Ok(self
            .root()?
            .pointer("/env/ANTHROPIC_BASE_URL")
            .and_then(|v| v.as_str().map(str::to_string)))
    }

    fn autostart_present(&self) -> Result<bool> {
        Ok(session_start_entry_present(&self.root()?))
    }

    fn manual_line(&self, base_url: &str) -> String {
        format!("export ANTHROPIC_BASE_URL={base_url}")
    }

    fn write(&self, base_url: &str, autostart: bool) -> Result<Vec<String>> {
        let mut root = self.root()?;
        {
            let obj = root
                .as_object_mut()
                .context("settings.json root is not an object")?;
            let env = obj
                .entry("env")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .context("settings.json `env` is not an object")?;
            env.insert("ANTHROPIC_BASE_URL".into(), serde_json::json!(base_url));
        }
        if autostart && !session_start_entry_present(&root) {
            insert_session_start_entry(&mut root)?;
        }
        let serialized = serde_json::to_string_pretty(&root)?;
        let backup = backup_and_atomic_write(&self.settings_path, &serialized)?;
        let mut lines = vec![format!(
            "  settings.json: env.ANTHROPIC_BASE_URL = {base_url}"
        )];
        if let Some(path) = backup {
            lines.push(format!("  Backup: {}", path.display()));
        }
        if autostart {
            lines.push(format!("  {}", CLAUDE_SPEC.autostart_done));
        }
        Ok(lines)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Persistence {
    Autostart,
    Service,
    Manual,
}

pub(super) fn offer_proxy_onramp(
    onramp: &dyn ProxyOnramp,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    use std::io::{BufRead, IsTerminal};
    if !matches!(patch_mode, PatchMode::Ask) {
        return Ok(());
    }
    if ctx.dry_run || !std::io::stdin().is_terminal() {
        return Ok(());
    }
    if crate::core::user_env::var("RTK_ONRAMP_DISABLED").as_deref() == Some("1") {
        return Ok(());
    }
    let spec = onramp.spec();
    let base_url = format!("http://{}{}", crate::llm_proxy::DEFAULT_BIND, onramp.path());
    let already = onramp.current_target()?;
    let aimed_here = already.as_deref() == Some(base_url.as_str());
    let autostart_present = onramp.autostart_present()?;
    if aimed_here && (autostart_present || spec.autostart_option.is_none()) {
        return Ok(());
    }
    if !crate::llm_proxy::binary_available() {
        eprintln!();
        eprintln!(
            "  RTK proxy not offered: rtk-proxy is not installed (`rtk llm-proxy start` says where to get it)."
        );
        return Ok(());
    }

    if aimed_here {
        eprintln!();
        eprintln!(
            "  The RTK proxy is configured in {}, but nothing restarts the daemon after a reboot.",
            spec.config
        );
    } else {
        eprintln!();
        if let Some(foreign) = already.as_deref() {
            eprintln!(
                "  {} already routes {} through {foreign} — another proxy or gateway.",
                spec.config, spec.label
            );
            eprint!("  Replace it with the RTK proxy? Your current routing stops working. [y/N] ");
        } else {
            eprint!(
                "  Set up the RTK proxy (routes {}'s API traffic through a local daemon)? [y/N] ",
                spec.label
            );
        }
        let mut line = String::new();
        let n = std::io::stdin().lock().read_line(&mut line)?;
        if n == 0 || !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("  Skipped — enable later with `rtk init -g` or `rtk llm-proxy start --env`.");
            return Ok(());
        }
    }

    let mut options: Vec<(Persistence, String)> = Vec::new();
    if let Some(text) = spec.autostart_option {
        options.push((Persistence::Autostart, text.to_string()));
    }
    options.push((
        Persistence::Service,
        "Background service — `rtk llm-proxy service install`".to_string(),
    ));
    options.push((Persistence::Manual, spec.manual_option.to_string()));
    eprintln!("  How should the daemon come back after a reboot?");
    for (i, (_, text)) in options.iter().enumerate() {
        eprintln!("    {}) {text}", i + 1);
    }
    eprint!(
        "  [{}, default 1] ",
        (1..=options.len())
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("/")
    );
    let mut choice_line = String::new();
    let n = std::io::stdin().lock().read_line(&mut choice_line)?;
    if n == 0 {
        println!("  No answer — nothing changed. Re-run `rtk init -g` to decide later.");
        return Ok(());
    }
    let choice = match choice_line.trim() {
        "" => 1,
        other => match other.parse::<usize>() {
            Ok(n) if (1..=options.len()).contains(&n) => n,
            _ => {
                println!("  Unrecognized `{other}` — nothing changed.");
                return Ok(());
            }
        },
    };
    let persistence = options[choice - 1].0;

    match persistence {
        Persistence::Manual => {
            println!(
                "  {} left untouched. Point a session at the daemon with:",
                spec.config
            );
            println!("    {}", onramp.manual_line(&base_url));
        }
        Persistence::Autostart | Persistence::Service => {
            for line in onramp.write(&base_url, persistence == Persistence::Autostart)? {
                println!("{line}");
            }
        }
    }

    match ensure_pipeline_extends("rtk:proxy", "") {
        PipelineChange::Created(path) => {
            println!("  {path}: created with extends = \"rtk:proxy\"");
        }
        PipelineChange::AlreadyActive => {}
        PipelineChange::ManualNeeded => {
            println!(
                "  Add `extends = \"rtk:proxy\"` to [pipeline] in your pipeline.toml — \
                 without it the daemon forwards unchanged."
            );
        }
    }

    match crate::llm_proxy::ensure_running(crate::llm_proxy::DEFAULT_BIND) {
        Ok(_) => println!("  rtk-proxy: running"),
        Err(e) => {
            println!("  [warn] rtk-proxy not started: {e}");
            println!("  Start it manually: rtk llm-proxy start");
        }
    }
    if persistence == Persistence::Service {
        let exe = std::env::current_exe().context("resolve the rtk binary path")?;
        match std::process::Command::new(exe)
            .args(["llm-proxy", "service", "install"])
            .status()
        {
            Ok(s) if s.success() => println!("  service: installed"),
            Ok(s) => println!("  [warn] service install exited with {s}"),
            Err(e) => println!("  [warn] service install failed: {e}"),
        }
    }
    match persistence {
        Persistence::Autostart => {
            verify_proxy_setup(onramp, &base_url, Some(spec.autostart_persistence))
        }
        Persistence::Service => verify_proxy_setup(onramp, &base_url, Some("background service")),
        Persistence::Manual => println!("  Keep it across logins: rtk llm-proxy service install"),
    }
    Ok(())
}

fn verify_proxy_setup(onramp: &dyn ProxyOnramp, base_url: &str, persistence: Option<&str>) {
    let target = onramp.current_target().ok().flatten();
    for line in proxy_verification_lines(
        onramp.spec(),
        base_url,
        crate::llm_proxy::is_reachable(),
        target.as_deref(),
        persistence,
    ) {
        println!("{line}");
    }
}

fn proxy_verification_lines(
    spec: &OnrampSpec,
    base_url: &str,
    reachable: bool,
    target: Option<&str>,
    persistence: Option<&str>,
) -> Vec<String> {
    let aimed_here = target == Some(base_url);
    let mut out = vec![
        format!(
            "  verify: daemon {}",
            if reachable {
                "responding".to_string()
            } else {
                format!("NOT responding on {}", crate::llm_proxy::DEFAULT_BIND)
            }
        ),
        format!(
            "  verify: {} {}",
            spec.config,
            match target {
                Some(t) if aimed_here => format!("{} = {t}", spec.key),
                Some(t) => format!("{} = {t} (not this daemon)", spec.key),
                None => format!("{} missing", spec.key),
            }
        ),
    ];
    if reachable && aimed_here {
        out.push(format!(
            "  proxy: configured — restart {}, then `rtk gain` shows requests",
            spec.label
        ));
        out.push(match persistence {
            Some(p) => format!("  verify: restart persistence — {p}"),
            None => "  [warn] nothing restarts the daemon after a reboot — rerun `rtk init -g`"
                .to_string(),
        });
        out.push(
            "  Compression: cache-breakpoint alignment — a no-op for clients that set their own, Claude Code included; CCR ships off (`rtk pipeline show rtk:proxy`)"
                .to_string(),
        );
    } else {
        out.push(
            "  proxy: NOT fully configured — `rtk gain` will report no proxy data".to_string(),
        );
    }
    out
}

/// True when any SessionStart entry already carries the rtk claude hook.
fn session_start_entry_present(root: &serde_json::Value) -> bool {
    root.pointer(&format!(
        "/hooks/{}",
        crate::hooks::constants::SESSION_START_KEY
    ))
    .and_then(|v| v.as_array())
    .is_some_and(|arr| {
        arr.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(is_claude_hook_command)
                    })
                })
        })
    })
}

/// SessionStart entries carry no matcher: the hook fires for every session.
fn insert_session_start_entry(root: &mut serde_json::Value) -> Result<()> {
    let obj = root
        .as_object_mut()
        .context("settings.json root is not an object")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("settings.json `hooks` is not an object")?;
    let arr = hooks
        .entry(crate::hooks::constants::SESSION_START_KEY)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .context("settings.json `SessionStart` is not an array")?;
    arr.push(serde_json::json!({
        "hooks": [{ "type": "command", "command": "rtk hook claude" }]
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "http://127.0.0.1:8080";

    #[test]
    fn a_daemon_nothing_points_at_is_not_configured() {
        let lines = proxy_verification_lines(&CLAUDE_SPEC, BASE, true, None, None).join("\n");
        assert!(lines.contains("env.ANTHROPIC_BASE_URL missing"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[test]
    fn an_env_var_aimed_at_a_dead_daemon_is_not_configured() {
        let lines =
            proxy_verification_lines(&CLAUDE_SPEC, BASE, false, Some(BASE), None).join("\n");
        assert!(lines.contains("NOT responding"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[test]
    fn an_env_var_aimed_elsewhere_is_named_rather_than_accepted() {
        let other = "http://127.0.0.1:9999";
        let lines =
            proxy_verification_lines(&CLAUDE_SPEC, BASE, true, Some(other), None).join("\n");
        assert!(lines.contains("not this daemon"), "{lines}");
        assert!(lines.contains("NOT fully configured"), "{lines}");
    }

    #[test]
    fn configured_state_warns_when_nothing_restarts_the_daemon() {
        let lines = proxy_verification_lines(&CLAUDE_SPEC, BASE, true, Some(BASE), None).join("\n");
        assert!(lines.contains("nothing restarts the daemon"), "{lines}");
        let lines = proxy_verification_lines(
            &CLAUDE_SPEC,
            BASE,
            true,
            Some(BASE),
            Some("SessionStart autostart"),
        )
        .join("\n");
        assert!(
            lines.contains("restart persistence — SessionStart autostart"),
            "{lines}"
        );
        assert!(!lines.contains("nothing restarts"), "{lines}");
    }

    #[test]
    fn session_start_entry_roundtrip() {
        let mut root = serde_json::json!({});
        assert!(!session_start_entry_present(&root));
        insert_session_start_entry(&mut root).expect("insert");
        assert!(session_start_entry_present(&root));
        let entry = root
            .pointer("/hooks/SessionStart/0/hooks/0/command")
            .and_then(|v| v.as_str())
            .expect("command written");
        assert_eq!(entry, "rtk hook claude");
        assert!(
            root.pointer("/hooks/SessionStart/0/matcher").is_none(),
            "SessionStart entries carry no matcher"
        );
        assert!(claude::remove_hook_from_json(&mut root));
        assert!(!session_start_entry_present(&root));
        assert!(
            root.get("hooks").is_none(),
            "uninstall must drop the emptied hooks object: {root}"
        );
    }

    #[test]
    fn foreign_session_start_hooks_survive_uninstall() {
        let mut root = serde_json::json!({
            "hooks": { "SessionStart": [
                { "hooks": [{ "type": "command", "command": "my-own-tool --hello" }] }
            ]}
        });
        insert_session_start_entry(&mut root).expect("insert");
        assert!(claude::remove_hook_from_json(&mut root));
        let arr = root
            .pointer("/hooks/SessionStart")
            .and_then(|v| v.as_array())
            .expect("foreign entry must survive");
        assert_eq!(arr.len(), 1, "{root}");
        assert!(arr[0].to_string().contains("my-own-tool"), "{root}");
    }

    #[test]
    fn both_halves_holding_is_the_only_configured_state() {
        let lines = proxy_verification_lines(&CLAUDE_SPEC, BASE, true, Some(BASE), None).join("\n");
        assert!(lines.contains("proxy: configured"), "{lines}");
        assert!(lines.contains("restart Claude Code"), "{lines}");
        assert!(!lines.contains("NOT"), "{lines}");
        assert!(lines.contains("CCR ships off"), "{lines}");
    }

    #[test]
    fn the_claude_onramp_writes_the_env_and_the_session_start_hook_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            "{\"permissions\":{\"allow\":[\"Bash(ls)\"]}}",
        )
        .expect("seed");
        let onramp = ClaudeOnramp {
            settings_path: settings_path.clone(),
        };
        assert_eq!(onramp.current_target().unwrap(), None);
        assert!(!onramp.autostart_present().unwrap());

        let lines = onramp.write(BASE, true).expect("write");
        assert!(
            lines[0].contains("env.ANTHROPIC_BASE_URL = http://127.0.0.1:8080"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("SessionStart hook registered")),
            "{lines:?}"
        );
        assert_eq!(onramp.current_target().unwrap().as_deref(), Some(BASE));
        assert!(onramp.autostart_present().unwrap());

        onramp.write(BASE, true).expect("rewrite");
        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            root.pointer("/hooks/SessionStart")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(root.pointer("/permissions/allow/0").unwrap(), "Bash(ls)");
        assert_eq!(
            onramp.manual_line(BASE),
            "export ANTHROPIC_BASE_URL=http://127.0.0.1:8080"
        );
    }
}
