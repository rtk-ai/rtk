//! Offline Desktop CMD external-command manifest. Its checked-in source fixture
//! is tests/fixtures/windows_cmd/windows_commands_az.tsv; no build or runtime fetches it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provenance {
    MicrosoftWindowsCommandsAz20250729,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionStatus {
    Supported,
    Deprecated,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Presence {
    Inbox,
    OptionalFeature,
    SeparateInstall,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseSupport {
    pub status: VersionStatus,
    pub presence: Presence,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Win11Support {
    pub before_24h2: ReleaseSupport,
    pub from_24h2: ReleaseSupport,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Win10Support {
    pub before_21h1: ReleaseSupport,
    pub from_21h1: ReleaseSupport,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopSupport {
    pub win10: Win10Support,
    pub win11: Win11Support,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalRoute {
    NativeExecutable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandModes(u8);
#[allow(dead_code)]
impl CommandModes {
    pub const QUERY: Self = Self(1);
    pub const MUTATION: Self = Self(2);
    pub const INTERACTIVE: Self = Self(4);
    pub const STRUCTURED: Self = Self(8);
    pub const MACHINE: Self = Self(16);
    pub const CONSERVATIVE_ANY: Self = Self(31);
    pub const fn union(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }
    #[cfg(test)]
    pub const fn contains(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalStrategy {
    IdentityRaw,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalStatus {
    RecognizedRaw,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub route: ExternalRoute,
    pub desktop: DesktopSupport,
    pub modes: CommandModes,
    pub strategy: ExternalStrategy,
    pub identity_reason: &'static str,
    pub status: ExternalStatus,
    pub provenance: Provenance,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogDisposition {
    DesktopExternal,
    CmdBuiltin,
    UnsupportedOnDesktop,
    OptionalDesktopFeature,
    SeparateInstall,
    VersionConditional,
    ServerOnly,
    SubcommandOnly,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogCoverage {
    pub source_name: &'static str,
    pub normalized_name: &'static str,
    pub disposition: CatalogDisposition,
    pub provenance: Provenance,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OfficialSourceMetadata {
    pub source_url: &'static str,
    pub fetched_on: &'static str,
    pub source_sha256: &'static str,
    pub format: &'static str,
}
#[cfg(test)]
pub const OFFICIAL_SOURCE_RAW_SHA256: &str =
    "b177c3014e3fa42294ed6fd5356d4bfa08e1a58a8b841e383dd5bcdc01837cc7";
#[cfg(test)]
pub const OFFICIAL_SOURCE_ENTRY_COUNT: usize = 339;
#[cfg(test)]
pub const OFFICIAL_SOURCE_FIXTURE_SHA256: &str =
    "b5923b7a56ae1524664b117d972b05f253096568f892860afd234f3a3c3b19a1";
const SOURCE: Provenance = Provenance::MicrosoftWindowsCommandsAz20250729;
const fn release(status: VersionStatus, presence: Presence) -> ReleaseSupport {
    ReleaseSupport { status, presence }
}
const fn uniform_desktop(presence: Presence) -> DesktopSupport {
    let support = release(VersionStatus::Supported, presence);
    DesktopSupport {
        win10: Win10Support {
            before_21h1: support,
            from_21h1: support,
        },
        win11: Win11Support {
            before_24h2: support,
            from_24h2: support,
        },
    }
}
const D: DesktopSupport = uniform_desktop(Presence::Inbox);
const O: DesktopSupport = uniform_desktop(Presence::OptionalFeature);
const S: DesktopSupport = uniform_desktop(Presence::SeparateInstall);
const W: DesktopSupport = DesktopSupport {
    win10: Win10Support {
        before_21h1: release(VersionStatus::Supported, Presence::Inbox),
        from_21h1: release(VersionStatus::Deprecated, Presence::Inbox),
    },
    win11: Win11Support {
        before_24h2: release(VersionStatus::Deprecated, Presence::OptionalFeature),
        from_24h2: release(VersionStatus::Unsupported, Presence::Unavailable),
    },
};
macro_rules! x {($n:literal,$d:expr,$_m:expr)=>{ExternalCommand{name:$n,aliases:&[],route:ExternalRoute::NativeExecutable,desktop:$d,modes:ANY,strategy:ExternalStrategy::IdentityRaw,identity_reason:"no external adapter is released in the stable CMD increment",status:ExternalStatus::RecognizedRaw,provenance:SOURCE}};($n:literal,[$($a:literal),+],$d:expr,$_m:expr)=>{ExternalCommand{name:$n,aliases:&[$($a),+],route:ExternalRoute::NativeExecutable,desktop:$d,modes:ANY,strategy:ExternalStrategy::IdentityRaw,identity_reason:"no external adapter is released in the stable CMD increment",status:ExternalStatus::RecognizedRaw,provenance:SOURCE}}}
#[allow(dead_code)]
const Q: CommandModes = CommandModes::QUERY;
#[allow(dead_code)]
const QM: CommandModes = Q.union(CommandModes::MACHINE);
#[allow(dead_code)]
const QMUT: CommandModes = Q.union(CommandModes::MUTATION);
#[allow(dead_code)]
const QMUTSM: CommandModes = QMUT
    .union(CommandModes::STRUCTURED)
    .union(CommandModes::MACHINE);
const ANY: CommandModes = CommandModes::CONSERVATIVE_ANY;
macro_rules! documented {($($n:literal),* ; $($extra:expr),* $(,)?)=>{&[$(x!($n,D,ANY),)* $($extra),*]}}
pub static EXTERNAL_COMMANDS: &[ExternalCommand] = documented!("arp","attrib","auditpol","autochk","bcdboot","bdehdcfg","bitsadmin","cacls","certreq","chkdsk","chkntfs","choice","cipher","cleanmgr","clip","cmd","cmdkey","cmstp","comp","compact","convert","cscript","defrag","diantz","diskcomp","diskcopy","diskpart","diskperf","diskshadow","dispdiag","doskey","driverquery","eventcreate","expand","fc","find","findstr","fondue","forfiles","format","fsutil","ftp","fveupdate","getmac","gpresult","gpupdate","hostname","icacls","klist","label","lodctr","logman","logoff","makecab","manage-bde","mmc","mode","more","mountvol","msg","msiexec","msinfo32","mstsc","nbtstat","netcfg","netsh","netstat","nslookup","openfiles","perfmon","pktmon","pnputil","powershell","powershell_ise","print","rdpsign","recover","regini","regsvr32","relog","replace","robocopy","rpcping","rundll32","rwinsta","schtasks","secedit","setspn","setx","sfc","shadow","shutdown","sort","subst","sxstrace","systeminfo","takeown","taskkill","tasklist","timeout","tpmtool","tpmvscmgr","tracerpt","tree","tscon","tsdiscon","tskill","typeperf","tzutil","unlodctr","verifier","vssadmin","waitfor","wbadmin","wecutil","where","whoami","winrs","winsat","wscript","xcopy";
 x!("change",["chglogon","chgport","chgusr"],D,Q),x!("query",["qappsrv","qprocess","quser","qwinsta"],D,Q),
 x!("ipconfig",D,QM),x!("ping",D,QM),x!("pathping",D,QM),x!("tracert",D,QM),
 x!("net",D,ANY),x!("sc",D,ANY),x!("route",D,ANY),
 x!("reg",D,QMUTSM),x!("bcdedit",D,QMUTSM),x!("certutil",D,QMUTSM),x!("wevtutil",D,QMUTSM),
 x!("mount",O,ANY),x!("telnet",O,ANY),x!("tftp",O,QM),x!("finger",O,ANY),x!("rsh",O,ANY),x!("showmount",O,ANY),
 x!("dtrace",S,ANY),x!("pwsh",S,ANY),x!("sysmon",S,ANY),x!("wmic",W,QMUTSM));
pub fn classify_external(name: &str) -> Option<&'static ExternalCommand> {
    EXTERNAL_COMMANDS.iter().find(|e| {
        e.name.eq_ignore_ascii_case(name) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    })
}
#[cfg(test)]
pub const fn external_commands() -> &'static [ExternalCommand] {
    EXTERNAL_COMMANDS
}
#[cfg(test)]
pub fn official_source_metadata() -> Result<OfficialSourceMetadata, String> {
    let fixture = include_str!("../../../tests/fixtures/windows_cmd/windows_commands_az.tsv");
    let mut source_url = None;
    let mut fetched_on = None;
    let mut source_sha256 = None;
    let mut format = None;
    for line in fixture.lines().take_while(|line| line.starts_with('#')) {
        let Some((key, value)) = line
            .strip_prefix("# ")
            .and_then(|line| line.split_once(": "))
        else {
            return Err(format!("malformed official fixture header: {line}"));
        };
        let field = match key {
            "source-url" => &mut source_url,
            "fetched-on" => &mut fetched_on,
            "source-sha256" => &mut source_sha256,
            "format" => &mut format,
            _ => return Err(format!("unknown official fixture header field: {key}")),
        };
        if field.replace(value).is_some() {
            return Err(format!("duplicate official fixture header field: {key}"));
        }
    }
    Ok(OfficialSourceMetadata {
        source_url: source_url.ok_or("missing source-url header")?,
        fetched_on: fetched_on.ok_or("missing fetched-on header")?,
        source_sha256: source_sha256.ok_or("missing source-sha256 header")?,
        format: format.ok_or("missing format header")?,
    })
}
#[cfg(test)]
pub fn official_top_level_coverage() -> &'static [CatalogCoverage] {
    use std::sync::OnceLock;
    static C: OnceLock<Vec<CatalogCoverage>> = OnceLock::new();
    C.get_or_init(|| {
        include_str!("../../../tests/fixtures/windows_cmd/windows_commands_az.tsv")
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let mut f = l.split('\t');
                let source_name = f.next().unwrap();
                let normalized_name = f.next().unwrap();
                let disposition = match f.next().unwrap() {
                    "desktop-external" => CatalogDisposition::DesktopExternal,
                    "cmd-builtin" => CatalogDisposition::CmdBuiltin,
                    "unsupported-desktop" => CatalogDisposition::UnsupportedOnDesktop,
                    "optional-feature" => CatalogDisposition::OptionalDesktopFeature,
                    "separate-install" => CatalogDisposition::SeparateInstall,
                    "version-conditional" => CatalogDisposition::VersionConditional,
                    "server-only" => CatalogDisposition::ServerOnly,
                    "subcommand-only" => CatalogDisposition::SubcommandOnly,
                    _ => panic!("bad disposition"),
                };
                assert!(f.next().is_none());
                CatalogCoverage {
                    source_name,
                    normalized_name,
                    disposition,
                    provenance: SOURCE,
                }
            })
            .collect()
    })
    .as_slice()
}
#[cfg(test)]
pub fn validate_external_manifest() -> Result<(), String> {
    let c = official_top_level_coverage();
    validate_external_manifest_rows(c, EXTERNAL_COMMANDS)
}

#[cfg(test)]
pub fn validate_external_manifest_rows(
    c: &[CatalogCoverage],
    external_commands: &[ExternalCommand],
) -> Result<(), String> {
    let builtins = super::catalog::builtins();
    let mut n = std::collections::HashSet::new();
    for e in external_commands {
        for name in std::iter::once(e.name).chain(e.aliases.iter().copied()) {
            if !n.insert(name.to_ascii_lowercase()) {
                return Err(format!("duplicate external command name or alias: {name}"));
            }
        }
        if !c.iter().any(|r| {
            r.normalized_name.eq_ignore_ascii_case(e.name)
                && matches!(
                    r.disposition,
                    CatalogDisposition::DesktopExternal
                        | CatalogDisposition::OptionalDesktopFeature
                        | CatalogDisposition::SeparateInstall
                        | CatalogDisposition::VersionConditional
                        | CatalogDisposition::SubcommandOnly
                )
        }) {
            return Err(format!("{} absent", e.name));
        }
    }
    let mut s = std::collections::HashSet::new();
    for r in c {
        if !s.insert(r.source_name.to_ascii_lowercase()) {
            return Err(format!("duplicate {}", r.source_name));
        }
        if r.disposition == CatalogDisposition::CmdBuiltin {
            if !builtins
                .iter()
                .any(|builtin| builtin.matches(r.normalized_name))
            {
                return Err(format!(
                    "{} must resolve to an actual CMD builtin: {}",
                    r.source_name, r.normalized_name
                ));
            }
        } else if r.disposition == CatalogDisposition::SubcommandOnly {
            let target = r.normalized_name;
            let resolves_to_builtin = builtins.iter().any(|builtin| builtin.matches(target));
            let resolves_to_external = external_commands.iter().any(|external| {
                external.name.eq_ignore_ascii_case(target)
                    || external
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(target))
            });
            let resolves_to_canonical_family = c.iter().any(|candidate| {
                candidate.source_name.eq_ignore_ascii_case(target)
                    && candidate.normalized_name.eq_ignore_ascii_case(target)
                    && candidate.disposition != CatalogDisposition::SubcommandOnly
            });
            if !(resolves_to_builtin || resolves_to_external || resolves_to_canonical_family) {
                return Err(format!(
                    "{} has unresolved command-family target: {target}",
                    r.source_name
                ));
            }
        }
        let expected_desktop = match r.disposition {
            CatalogDisposition::DesktopExternal => Some(D),
            CatalogDisposition::OptionalDesktopFeature => Some(O),
            CatalogDisposition::SeparateInstall => Some(S),
            CatalogDisposition::VersionConditional => Some(W),
            _ => None,
        };
        if let Some(expected_desktop) = expected_desktop {
            let Some(external) = external_commands.iter().find(|external| {
                external.name.eq_ignore_ascii_case(r.normalized_name)
                    || external
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(r.normalized_name))
            }) else {
                return Err(format!("{} lacks external", r.source_name));
            };
            if external.desktop != expected_desktop {
                return Err(format!(
                    "{} disposition does not match {} release metadata",
                    r.source_name, external.name
                ));
            }
        }
    }
    Ok(())
}
