//! Checked-in metadata for CMD intrinsic commands and command extensions.
#![allow(dead_code)] // Consumed by the CMD orchestration and adapter tasks.

use std::collections::HashSet;

use super::external_manifest::{
    ExternalCommand, ExternalRoute, ExternalStatus, ExternalStrategy, Presence, Provenance,
    ReleaseSupport, VersionStatus,
};

/// Whether CMD itself provides the command or it depends on command extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinOrigin {
    Intrinsic,
    Extension,
}

/// Behavioral class used by the CMD orchestrator to preserve shell semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandMode {
    Query,
    Mutation,
    Stateful,
    Control,
    Interactive,
}

/// The adapter choice is explicit even when filtering would be unsafe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterStrategy {
    Identity { reason: &'static str },
    Structured { adapter: &'static str },
}

/// Metadata for a CMD command and all names that resolve to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltinCommand {
    pub name: &'static str,
    pub aliases: Vec<&'static str>,
    pub origin: BuiltinOrigin,
    pub mode: CommandMode,
    pub strategy: Option<AdapterStrategy>,
}

impl BuiltinCommand {
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
    }

    #[cfg(test)]
    pub fn with_aliases(mut self, aliases: &[&'static str]) -> Self {
        self.aliases = aliases.to_vec();
        self
    }

    #[cfg(test)]
    pub fn without_strategy(mut self) -> Self {
        self.strategy = None;
        self
    }
}

/// Return CMD commands available on supported Desktop Windows versions.
pub fn builtins() -> Vec<BuiltinCommand> {
    use AdapterStrategy::{Identity, Structured};
    use BuiltinOrigin::{Extension, Intrinsic};
    use CommandMode::{Control, Interactive, Mutation, Query, Stateful};

    vec![
        command(
            "assoc",
            &[],
            Intrinsic,
            Query,
            Structured { adapter: "assoc" },
        ),
        command(
            "break",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes CTRL+C handling",
            },
        ),
        command(
            "call",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "invokes batch control flow",
            },
        ),
        command(
            "cd",
            &["chdir"],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes current directory",
            },
        ),
        command(
            "chcp",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes console code page",
            },
        ),
        command(
            "cls",
            &[],
            Intrinsic,
            Query,
            Identity {
                reason: "console-only output",
            },
        ),
        command(
            "color",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes console attributes",
            },
        ),
        command(
            "copy",
            &[],
            Intrinsic,
            Mutation,
            Identity {
                reason: "file mutation",
            },
        ),
        command(
            "date",
            &[],
            Intrinsic,
            Interactive,
            Identity {
                reason: "may prompt for input",
            },
        ),
        command(
            "del",
            &["erase"],
            Intrinsic,
            Mutation,
            Identity {
                reason: "file mutation",
            },
        ),
        command("dir", &[], Intrinsic, Query, Structured { adapter: "dir" }),
        command(
            "echo",
            &[],
            Intrinsic,
            Query,
            Identity {
                reason: "output is requested verbatim",
            },
        ),
        command(
            "endlocal",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "restores environment scope",
            },
        ),
        command(
            "exit",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "terminates the shell or batch",
            },
        ),
        command(
            "for",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "expands and executes control flow",
            },
        ),
        command(
            "ftype",
            &[],
            Intrinsic,
            Query,
            Structured { adapter: "ftype" },
        ),
        command(
            "goto",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "batch control flow",
            },
        ),
        command(
            "help",
            &[],
            Extension,
            Query,
            Structured { adapter: "help" },
        ),
        command(
            "if",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "conditional control flow",
            },
        ),
        command(
            "md",
            &["mkdir"],
            Intrinsic,
            Mutation,
            Identity {
                reason: "creates directories",
            },
        ),
        command(
            "mklink",
            &[],
            Extension,
            Mutation,
            Identity {
                reason: "creates filesystem links",
            },
        ),
        command(
            "move",
            &[],
            Intrinsic,
            Mutation,
            Identity {
                reason: "moves filesystem entries",
            },
        ),
        command(
            "path",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes command search path",
            },
        ),
        command(
            "pause",
            &[],
            Intrinsic,
            Interactive,
            Identity {
                reason: "waits for console input",
            },
        ),
        command(
            "popd",
            &[],
            Extension,
            Stateful,
            Identity {
                reason: "changes current directory",
            },
        ),
        command(
            "prompt",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes prompt state",
            },
        ),
        command(
            "pushd",
            &[],
            Extension,
            Stateful,
            Identity {
                reason: "changes current directory",
            },
        ),
        command(
            "rd",
            &["rmdir"],
            Intrinsic,
            Mutation,
            Identity {
                reason: "removes directories",
            },
        ),
        command(
            "rem",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "batch comment syntax",
            },
        ),
        command(
            "ren",
            &["rename"],
            Intrinsic,
            Mutation,
            Identity {
                reason: "renames filesystem entries",
            },
        ),
        command(
            "set",
            &[],
            Intrinsic,
            Stateful,
            Structured { adapter: "set" },
        ),
        command(
            "setlocal",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "creates environment scope",
            },
        ),
        command(
            "shift",
            &[],
            Intrinsic,
            Control,
            Identity {
                reason: "changes batch arguments",
            },
        ),
        command(
            "start",
            &[],
            Intrinsic,
            Interactive,
            Identity {
                reason: "launches programs or windows",
            },
        ),
        command(
            "time",
            &[],
            Intrinsic,
            Interactive,
            Identity {
                reason: "may prompt for input",
            },
        ),
        command(
            "title",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes console title",
            },
        ),
        command(
            "type",
            &[],
            Intrinsic,
            Query,
            Identity {
                reason: "content may be exact or binary",
            },
        ),
        command(
            "ver",
            &[],
            Intrinsic,
            Query,
            Identity {
                reason: "naturally terse output",
            },
        ),
        command(
            "verify",
            &[],
            Intrinsic,
            Stateful,
            Identity {
                reason: "changes verification state",
            },
        ),
        command(
            "vol",
            &[],
            Intrinsic,
            Query,
            Identity {
                reason: "naturally terse output",
            },
        ),
    ]
}

fn command(
    name: &'static str,
    aliases: &[&'static str],
    origin: BuiltinOrigin,
    mode: CommandMode,
    strategy: AdapterStrategy,
) -> BuiltinCommand {
    BuiltinCommand {
        name,
        aliases: aliases.to_vec(),
        origin,
        mode,
        strategy: Some(strategy),
    }
}

/// Verify catalog names and aliases are unique and every command selects an adapter.
pub fn validate_catalog(catalog: &[BuiltinCommand]) -> Result<(), String> {
    let mut names = HashSet::new();
    for command in catalog {
        if command.strategy.is_none() {
            return Err(format!("{} has no adapter strategy", command.name));
        }
        if let Some(AdapterStrategy::Structured { adapter }) = command.strategy {
            if !super::adapters::supports_adapter(adapter) {
                return Err(format!(
                    "{} has unknown structured adapter: {adapter}",
                    command.name
                ));
            }
        }
        for name in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            let normalized = name.to_ascii_lowercase();
            if !names.insert(normalized) {
                return Err(format!("duplicate CMD command name or alias: {name}"));
            }
        }
    }
    Ok(())
}

/// Verify the complete CMD catalog is unambiguous and explicit.
///
/// Built-ins and external commands share one command namespace under CMD, so
/// duplicate names and aliases are rejected across both checked-in catalogs.
pub fn validate_command_catalogs(
    builtin_catalog: &[BuiltinCommand],
    external_catalog: &[ExternalCommand],
) -> Result<(), String> {
    validate_catalog(builtin_catalog)?;

    let mut names = HashSet::new();
    for command in builtin_catalog {
        for name in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            names.insert(name.to_ascii_lowercase());
        }
    }

    for command in external_catalog {
        if command.route != ExternalRoute::NativeExecutable
            || command.strategy != ExternalStrategy::IdentityRaw
            || command.status != ExternalStatus::RecognizedRaw
            || command.provenance != Provenance::MicrosoftWindowsCommandsAz20250729
            || command.desktop.win10.before_21h1.status == VersionStatus::Unsupported
            || command.desktop.win11.before_24h2.status == VersionStatus::Unsupported
            || [
                command.desktop.win10.before_21h1,
                command.desktop.win10.from_21h1,
                command.desktop.win11.before_24h2,
                command.desktop.win11.from_24h2,
            ]
            .into_iter()
            .any(|release| !release_support_is_consistent(release))
            || command.modes.is_empty()
            || command.identity_reason.trim().is_empty()
        {
            return Err(format!("{} has incomplete external metadata", command.name));
        }

        for name in std::iter::once(command.name).chain(command.aliases.iter().copied()) {
            let normalized = name.to_ascii_lowercase();
            if !names.insert(normalized) {
                return Err(format!("duplicate CMD command name or alias: {name}"));
            }
        }
    }

    Ok(())
}

fn release_support_is_consistent(release: ReleaseSupport) -> bool {
    matches!(release.status, VersionStatus::Unsupported)
        == matches!(release.presence, Presence::Unavailable)
}
