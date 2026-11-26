//! What a pipeline needs before it can run, read from its own file: each local
//! service it declares under `[dependency.<name>]`, each loopback upstream it
//! calls without one, and the RTK daemon when it extends `rtk:proxy` or has
//! proxy rules of its own (the proxy rule RTK's default ships does not count).
//! [`settle`] lists what is missing, then installs, asks for or reports each
//! piece. The caller keeps the pipeline only when every need is met; otherwise
//! the pipelines the user already had stay, and [`alert`] says why and how to
//! try again. API keys are not needs: `print_auth_guidance` explains them.

use crate::core::deps::{self, Mode, Offered, Review, State};
use anyhow::Result;
use rtk_engine::pipeline::{DependencyDef, Resolved, SiteKind, local_listen, url_authority};

#[derive(Debug, Clone, PartialEq)]
pub enum Need {
    Dependency {
        name: String,
        def: Box<DependencyDef>,
    },
    Service {
        upstream: String,
        address: String,
    },
    Daemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    Ask,
    Auto,
    NoTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Met,
    Declined(Vec<String>),
    Unmet(Vec<String>),
}

impl Settled {
    pub fn exit_code(&self) -> i32 {
        match self {
            Settled::Unmet(_) => 1,
            Settled::Met | Settled::Declined(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Met,
    Pending(String),
    Blocked(String),
}

pub fn of(resolved: &Resolved, pipeline: &str) -> Vec<Need> {
    let mut out = Vec::new();
    for (name, (def, origin)) in &resolved.dependencies {
        if origin == pipeline {
            out.push(Need::Dependency {
                name: name.clone(),
                def: Box::new(def.clone()),
            });
        }
    }
    for (name, (def, origin)) in &resolved.upstreams {
        if origin != pipeline {
            continue;
        }
        let linked = def
            .dependency
            .as_ref()
            .is_some_and(|d| resolved.dependencies.contains_key(d));
        if linked {
            continue;
        }
        if let Some(address) = def
            .base_url
            .as_deref()
            .and_then(url_authority)
            .filter(|a| local_listen(a))
        {
            out.push(Need::Service {
                upstream: name.clone(),
                address: address.to_string(),
            });
        }
    }
    let own_proxy_rules = resolved
        .rules
        .iter()
        .filter(|(position, _)| position.site == SiteKind::Proxy)
        .flat_map(|(_, rules)| rules)
        .any(|r| r.origin == pipeline && r.def.enabled && !r.def.pro_inert);
    if own_proxy_rules || resolved.extends.iter().any(|e| e == "rtk:proxy") {
        out.push(Need::Daemon);
    }
    out
}

pub fn settle(
    needs: &[Need],
    pipeline: &str,
    review: &Review,
    consent: Consent,
    refusal: &str,
) -> Result<Settled> {
    let active = needs
        .iter()
        .any(|n| matches!(n, Need::Dependency { .. }))
        .then(crate::core::pipeline_cmd::load_for_cli);
    let statuses: Vec<Status> = needs
        .iter()
        .map(|need| {
            status(
                need,
                pipeline,
                review,
                consent,
                active.as_ref().map(|l| &l.resolved),
            )
        })
        .collect();
    if statuses.iter().all(|s| *s == Status::Met) {
        return Ok(Settled::Met);
    }
    println!();
    println!("  {pipeline} needs, to run:");
    for (need, status) in needs.iter().zip(&statuses) {
        let state = match status {
            Status::Met => "ready",
            Status::Pending(state) | Status::Blocked(state) => state.as_str(),
        };
        println!("    - {}: {state}", label(need));
    }
    let blocked: Vec<String> = needs
        .iter()
        .zip(&statuses)
        .filter_map(|(need, status)| match status {
            Status::Blocked(reason) => Some(format!("{}: {reason}", label(need))),
            _ => None,
        })
        .collect();
    if !blocked.is_empty() {
        return Ok(Settled::Unmet(blocked));
    }
    let mode = if consent == Consent::Ask {
        Mode::Ask
    } else {
        Mode::Auto
    };
    for (need, status) in needs.iter().zip(&statuses) {
        if !matches!(status, Status::Pending(_)) {
            continue;
        }
        match need {
            Need::Dependency { name, def } => {
                match deps::offer(pipeline, name, def, review, mode, refusal)? {
                    Offered::Ready | Offered::Installed => {}
                    Offered::Declined => return Ok(Settled::Declined(vec![label(need)])),
                    Offered::Failed(reason) | Offered::Unavailable(reason) => {
                        return Ok(Settled::Unmet(vec![format!("{}: {reason}", label(need))]));
                    }
                }
            }
            Need::Daemon => {
                if !crate::hooks::init::ask_daemon(pipeline)? {
                    return Ok(Settled::Declined(vec![label(need)]));
                }
            }
            Need::Service { .. } => {}
        }
    }
    Ok(Settled::Met)
}

pub fn alert(pipeline: &str, settled: &Settled, outcome: &str, running: &str, rerun: &str) {
    use colored::Colorize;
    let (missing, again) = match settled {
        Settled::Met => return,
        Settled::Declined(missing) => (missing, "If you answered by mistake, run"),
        Settled::Unmet(missing) => (missing, "Once that is fixed, run in a terminal"),
    };
    let mut lines = vec![format!("! {pipeline} {outcome}. It needs:")];
    lines.extend(missing.iter().map(|m| format!("    - {m}")));
    lines.push(format!("  RTK keeps running {running}."));
    lines.push(format!("  {again}: {rerun}"));
    println!();
    for line in lines {
        println!("  {}", line.yellow());
    }
}

pub fn running_now(excluding: &str) -> String {
    let mut names: Vec<String> = Vec::new();
    if crate::core::pipeline_config::user_pipeline_path().is_some_and(|p| p.exists()) {
        names.push(crate::core::constants::PIPELINE_TOML.to_string());
    }
    if let Some(dir) = crate::core::pipeline_config::user_pipelines_dir()
        && let Ok(read) = std::fs::read_dir(&dir)
    {
        let mut found: Vec<String> = read
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
            .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
            .filter(|name| name != excluding)
            .collect();
        found.sort();
        names.extend(found);
    }
    match names.len() {
        0 => "RTK's default pipeline".to_string(),
        1 => format!("your pipeline {}", names[0]),
        _ => format!("your pipelines {}", names.join(", ")),
    }
}

fn status(
    need: &Need,
    pipeline: &str,
    review: &Review,
    consent: Consent,
    active: Option<&Resolved>,
) -> Status {
    match need {
        Need::Dependency { name, def } => {
            if let Some((other, origin)) = active.and_then(|a| a.dependencies.get(name))
                && deps::pipeline_name(origin) != pipeline
                && deps::fingerprint(other) != deps::fingerprint(def)
            {
                return Status::Blocked(format!(
                    "your pipeline {} already uses a different {name}, and installing this one would replace it",
                    deps::pipeline_name(origin)
                ));
            }
            if let Err(reason) = deps::installable(def) {
                return Status::Blocked(format!(
                    "rtk cannot install it on this computer ({reason})"
                ));
            }
            let missing = match deps::state(name, def) {
                State::Installed => return Status::Met,
                State::Missing if deps::listen_answers(def) => return Status::Met,
                State::Outdated => "the installed copy differs from the one it declares",
                State::Missing => "not installed",
            };
            match consent {
                Consent::Ask => Status::Pending(missing.to_string()),
                Consent::Auto if matches!(review, Review::Reviewed(_)) => {
                    Status::Pending(missing.to_string())
                }
                Consent::Auto => Status::Blocked(format!(
                    "{missing}; rtk-ai has not reviewed this pipeline, so rtk installs it only after a yes in a terminal"
                )),
                Consent::NoTerminal => Status::Blocked(format!(
                    "{missing}; rtk installs it only after a yes in a terminal"
                )),
            }
        }
        Need::Service { address, .. } => {
            if deps::address_answers(address) {
                Status::Met
            } else {
                Status::Blocked(format!(
                    "nothing answers on {address}, and the pipeline does not say how to install it; start that service first"
                ))
            }
        }
        Need::Daemon => {
            if crate::hooks::init::daemon_ready() {
                Status::Met
            } else if let Some(reason) = crate::hooks::init::daemon_unavailable() {
                Status::Blocked(format!("not set up; {reason}"))
            } else if consent == Consent::Ask {
                Status::Pending("not set up".to_string())
            } else {
                Status::Blocked(
                    "not set up; rtk sets it up only after a yes in a terminal".to_string(),
                )
            }
        }
    }
}

fn label(need: &Need) -> String {
    match need {
        Need::Dependency { name, def } => match def.listen.as_deref() {
            Some(listen) => format!("{name}, a local service on {listen}"),
            None => format!("{name}, a local service"),
        },
        Need::Service { upstream, address } => {
            format!("the local service on {address} (upstream `{upstream}`)")
        }
        Need::Daemon => "the RTK daemon".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pipeline_config::load_candidate;

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    }

    #[test]
    fn needs_come_from_the_pipelines_own_file() {
        let text = r#"
schema_version = 1
[pipeline]
name = "p"
extends = ["rtk:default"]

[dependency.svc]
run = ["svc"]
listen = "127.0.0.1:41911"

[upstream.linked]
provider = "openai"
model = "m"
base_url = "http://127.0.0.1:41911/v1"
dependency = "svc"

[upstream.local]
provider = "openai"
model = "m"
base_url = "http://localhost:41912/v1"

[upstream.remote]
provider = "anthropic"
model = "claude-haiku"

[[proxy.request.tool_result]]
name = "trim"
use = ["ansi"]
"#;
        let loaded = load_candidate(text, "p");
        let needs = of(&loaded.resolved, "p");
        assert_eq!(
            needs
                .iter()
                .map(|n| match n {
                    Need::Dependency { name, .. } => format!("dependency {name}"),
                    Need::Service { upstream, address } => format!("service {upstream} {address}"),
                    Need::Daemon => "daemon".to_string(),
                })
                .collect::<Vec<_>>(),
            vec!["dependency svc", "service local localhost:41912", "daemon"]
        );
    }

    #[test]
    fn the_default_proxy_rule_alone_needs_no_daemon() {
        let text =
            "schema_version = 1\n[pipeline]\nname = \"plain\"\nextends = [\"rtk:default\"]\n";
        let loaded = load_candidate(text, "plain");
        assert!(of(&loaded.resolved, "plain").is_empty());
    }

    #[test]
    fn extending_the_proxy_pipeline_needs_the_daemon() {
        let text = "schema_version = 1\n[pipeline]\nname = \"cache\"\nextends = [\"rtk:default\", \"rtk:proxy\"]\n";
        let loaded = load_candidate(text, "cache");
        assert_eq!(of(&loaded.resolved, "cache"), vec![Need::Daemon]);
    }

    fn with_dependency(pipeline: &str, url: &str, listen: &str) -> String {
        format!(
            "schema_version = 1\n[pipeline]\nname = \"{pipeline}\"\n[dependency.svc-dup]\nrun = [\"svc\"]\nlisten = \"{listen}\"\n[dependency.svc-dup.binary.{}]\nurl = \"{url}\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n",
            deps::platform()
        )
    }

    #[test]
    fn a_dependency_another_pipeline_runs_differently_blocks_the_newcomer() {
        let listen = format!("127.0.0.1:{}", free_port());
        let other = load_candidate(
            &with_dependency("other", "https://example.com/svc-1", &listen),
            "other",
        );
        let mine = load_candidate(
            &with_dependency("mine", "https://example.com/svc-2", &listen),
            "mine",
        );
        let needs = of(&mine.resolved, "mine");
        let blocked = status(
            &needs[0],
            "mine",
            &Review::Unreviewed,
            Consent::Ask,
            Some(&other.resolved),
        );
        assert!(
            matches!(&blocked, Status::Blocked(r) if r.contains("your pipeline other already uses a different svc-dup")),
            "{blocked:?}"
        );
        let same = load_candidate(
            &with_dependency("same", "https://example.com/svc-2", &listen),
            "same",
        );
        let pending = status(
            &needs[0],
            "mine",
            &Review::Unreviewed,
            Consent::Ask,
            Some(&same.resolved),
        );
        assert_eq!(pending, Status::Pending("not installed".to_string()));
    }

    #[test]
    fn a_dependency_whose_address_already_answers_is_met() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let listen = listener.local_addr().expect("addr").to_string();
        let mine = load_candidate(
            &with_dependency("mine", "https://example.com/svc-3", &listen),
            "mine",
        );
        let needs = of(&mine.resolved, "mine");
        assert_eq!(
            status(
                &needs[0],
                "mine",
                &Review::Unreviewed,
                Consent::NoTerminal,
                None
            ),
            Status::Met
        );
    }

    #[test]
    fn nothing_to_meet_is_met() {
        assert_eq!(
            settle(&[], "p", &Review::Unreviewed, Consent::NoTerminal, "").expect("settle"),
            Settled::Met
        );
    }

    #[test]
    fn a_stopped_service_without_a_recipe_is_unmet_without_asking() {
        let address = format!("127.0.0.1:{}", free_port());
        let needs = vec![Need::Service {
            upstream: "judge".to_string(),
            address: address.clone(),
        }];
        let settled = settle(&needs, "p", &Review::Unreviewed, Consent::Ask, "").expect("settle");
        match settled {
            Settled::Unmet(missing) => {
                assert_eq!(missing.len(), 1);
                assert!(missing[0].contains(&address), "{missing:?}");
            }
            other => panic!("expected unmet, got {other:?}"),
        }
    }

    #[test]
    fn a_running_service_is_met() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();
        let needs = vec![Need::Service {
            upstream: "judge".to_string(),
            address,
        }];
        assert_eq!(
            settle(&needs, "p", &Review::Unreviewed, Consent::NoTerminal, "").expect("settle"),
            Settled::Met
        );
    }

    #[test]
    fn without_a_terminal_an_unreviewed_dependency_is_unmet() {
        let text = format!(
            "schema_version = 1\n[pipeline]\nname = \"u\"\n[dependency.never-installed-x]\nrun = [\"x\"]\nlisten = \"127.0.0.1:{}\"\n[dependency.never-installed-x.binary.{}]\nurl = \"https://example.com/x\"\nsha256 = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n",
            free_port(),
            deps::platform()
        );
        let loaded = load_candidate(&text, "u");
        let needs = of(&loaded.resolved, "u");
        assert_eq!(needs.len(), 1);
        for consent in [Consent::NoTerminal, Consent::Auto] {
            let settled = settle(&needs, "u", &Review::Unreviewed, consent, "").expect("settle");
            assert!(
                matches!(&settled, Settled::Unmet(m) if m[0].contains("only after a yes in a terminal")),
                "{consent:?}: {settled:?}"
            );
            assert_eq!(settled.exit_code(), 1);
        }
    }
}
