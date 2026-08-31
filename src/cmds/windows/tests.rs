use super::adapters::{filter_display, is_display_form, supports_adapter};
use super::catalog::{
    builtins, validate_catalog, validate_command_catalogs, AdapterStrategy, CommandMode,
};
use super::external_manifest::{
    classify_external, external_commands, official_source_metadata, official_top_level_coverage,
    validate_external_manifest, validate_external_manifest_rows, CatalogDisposition, CommandModes,
    ExternalCommand, ExternalRoute, ExternalStatus, ExternalStrategy, Presence, VersionStatus,
    OFFICIAL_SOURCE_ENTRY_COUNT, OFFICIAL_SOURCE_FIXTURE_SHA256, OFFICIAL_SOURCE_RAW_SHA256,
};
use super::orchestrator::{
    prepare_invocation as prepare_with_cmd, recognize_command, rewrite_expression,
    CommandRecognition, Invocation, SEGMENT_RUNNER,
};
use super::parser::{parse_expression, OpaqueReason, OperatorKind, Span};
use std::ffi::OsString;
use std::path::Path;

fn segment_text(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}

fn prepare_invocation(args: &[OsString]) -> anyhow::Result<Invocation> {
    prepare_with_cmd(args, Path::new(r"C:\Windows\System32\cmd.exe"))
}

#[test]
fn parses_top_level_chains_with_exact_replacement_spans() {
    let source = "  echo hello  && dir /b || type readme.txt";
    let parsed = parse_expression(source);

    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.span))
            .collect::<Vec<_>>(),
        ["echo hello", "dir /b", "type readme.txt"]
    );
    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.command_span))
            .collect::<Vec<_>>(),
        ["echo", "dir", "type"]
    );
    assert_eq!(parsed.opaque_reason, None);
}

#[test]
fn quotes_and_caret_escapes_keep_operators_inside_simple_segments() {
    let source = r#"echo "a & b" ^& literal & dir"#;
    let parsed = parse_expression(source);

    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.span))
            .collect::<Vec<_>>(),
        [r#"echo "a & b" ^& literal"#, "dir"]
    );
    assert_eq!(parsed.opaque_reason, None);
}

#[test]
fn single_quotes_do_not_quote_cmd_operators() {
    let source = "echo 'a & b' & dir";
    let parsed = parse_expression(source);

    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.span))
            .collect::<Vec<_>>(),
        ["echo 'a", "b'", "dir"]
    );
    assert_eq!(parsed.opaque_reason, None);
}

#[test]
fn preserves_crlf_and_variable_expansions_without_reformatting() {
    let source = "@echo %USERNAME%\r\ndir %CD%";
    let parsed = parse_expression(source);

    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.span))
            .collect::<Vec<_>>(),
        ["@echo %USERNAME%", "dir %CD%"]
    );
    assert_eq!(
        segment_text(source, parsed.segments[0].command_span),
        "echo"
    );
    assert_eq!(parsed.opaque_reason, None);
}

#[test]
fn crlf_is_a_cmd_command_boundary() {
    let source = "echo one\r\necho two";
    let parsed = parse_expression(source);

    assert_eq!(
        parsed
            .segments
            .iter()
            .map(|segment| segment_text(source, segment.span))
            .collect::<Vec<_>>(),
        ["echo one", "echo two"]
    );
    assert_eq!(parsed.opaque_reason, None);
}

#[test]
fn quoted_batch_path_is_opaque() {
    let source = r#""C:\Program Files\build.cmd" --release"#;
    let parsed = parse_expression(source);

    assert_eq!(parsed.opaque_reason, Some(OpaqueReason::BatchInvocation));
    assert_eq!(
        segment_text(source, parsed.segments[0].command_span),
        r#""C:\Program Files\build.cmd""#
    );
}

#[test]
fn empty_operands_around_chain_operators_are_malformed() {
    for source in [
        "& echo hi",
        "echo hi &",
        "&& echo hi",
        "echo hi &&",
        "|| echo hi",
        "echo hi ||",
    ] {
        assert_eq!(
            parse_expression(source).opaque_reason,
            Some(OpaqueReason::MalformedInput),
            "{source}"
        );
    }
}

#[test]
fn bare_at_prefix_is_malformed_without_panicking() {
    for source in ["@", "@ "] {
        let parsed = parse_expression(source);

        assert_eq!(
            parsed.opaque_reason,
            Some(OpaqueReason::MalformedInput),
            "{source:?}"
        );
        assert_eq!(parsed.segments.len(), 1, "{source:?}");
    }
}

#[test]
fn fails_open_for_opaque_or_malformed_cmd_constructs() {
    let cases = [
        ("dir | findstr rs", OpaqueReason::OutputPipeline),
        ("dir > listing.txt", OpaqueReason::OutputRedirection),
        ("(echo one & echo two)", OpaqueReason::ControlGroup),
        ("if exist file echo yes", OpaqueReason::ControlCommand),
        ("build.bat", OpaqueReason::BatchInvocation),
        ("echo !PATH!", OpaqueReason::DelayedExpansion),
        ("C:", OpaqueReason::DriveChange),
        ("echo \"unterminated", OpaqueReason::MalformedInput),
        ("echo trailing^", OpaqueReason::MalformedInput),
    ];

    for (source, reason) in cases {
        assert_eq!(
            parse_expression(source).opaque_reason,
            Some(reason),
            "{source}"
        );
    }
}

#[test]
fn builtin_catalog_has_a_strategy_for_every_intrinsic_and_extension() {
    let names = builtins()
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        [
            "assoc", "break", "call", "cd", "chcp", "cls", "color", "copy", "date", "del", "dir",
            "echo", "endlocal", "exit", "for", "ftype", "goto", "help", "if", "md", "mklink",
            "move", "path", "pause", "popd", "prompt", "pushd", "rd", "rem", "ren", "set",
            "setlocal", "shift", "start", "time", "title", "type", "ver", "verify", "vol",
        ]
    );
    assert!(builtins().iter().all(|command| match command.strategy {
        Some(AdapterStrategy::Identity { .. } | AdapterStrategy::Structured { .. }) => true,
        None => false,
    }));
}

#[test]
fn builtin_catalog_marks_stateful_control_and_interactive_commands() {
    let catalog = builtins();
    let find = |name| {
        catalog
            .iter()
            .find(|command| command.matches(name))
            .unwrap()
    };

    assert_eq!(find("chdir").name, "cd");
    assert_eq!(find("erase").name, "del");
    assert_eq!(find("mkdir").name, "md");
    assert_eq!(find("rmdir").name, "rd");
    assert_eq!(find("rename").name, "ren");
    assert_eq!(find("if").mode, CommandMode::Control);
    assert_eq!(find("setlocal").mode, CommandMode::Stateful);
    assert_eq!(find("pause").mode, CommandMode::Interactive);
}

#[test]
fn catalog_validation_rejects_duplicate_aliases_and_missing_strategies() {
    assert_eq!(validate_catalog(&builtins()), Ok(()));

    let duplicate = [
        builtins()[0].clone(),
        builtins()[1].clone().with_aliases(&["assoc"]),
    ];
    assert!(validate_catalog(&duplicate).is_err());

    let missing_strategy = [builtins()[0].clone().without_strategy()];
    assert!(validate_catalog(&missing_strategy).is_err());

    let mut unknown_adapter = builtins()[0].clone();
    unknown_adapter.strategy = Some(AdapterStrategy::Structured {
        adapter: "missing-adapter",
    });
    assert!(validate_catalog(&[unknown_adapter]).is_err());
}

#[test]
fn external_manifest_is_static_complete_and_explicit() {
    assert_eq!(
        validate_command_catalogs(&builtins(), external_commands()),
        Ok(())
    );
    assert_eq!(validate_external_manifest(), Ok(()));
    assert!(external_commands().len() >= 100);

    for entry in external_commands() {
        assert_eq!(entry.route, ExternalRoute::NativeExecutable);
        assert_eq!(entry.strategy, ExternalStrategy::IdentityRaw);
        assert_eq!(entry.status, ExternalStatus::RecognizedRaw);
        assert!(!entry.identity_reason.trim().is_empty(), "{}", entry.name);
        assert!(!entry.modes.is_empty(), "{}", entry.name);
        assert_ne!(
            entry.desktop.win10.before_21h1.status,
            VersionStatus::Unsupported,
            "{}",
            entry.name
        );
        assert_ne!(
            entry.desktop.win11.before_24h2.status,
            VersionStatus::Unsupported,
            "{}",
            entry.name
        );
    }

    let change = classify_external("chglogon").expect("change alias is recognized");
    assert_eq!(change.name, "change");
    assert!(change.aliases.contains(&"chgport"));
    let query = classify_external("qprocess").expect("query alias is recognized");
    assert_eq!(query.name, "query");
    assert!(query.modes.contains(CommandModes::QUERY));
    assert!(classify_external("not-a-windows-command").is_none());
    assert_eq!(
        recognize_command("ipconfig"),
        CommandRecognition::ExternalRaw
    );
    assert_eq!(
        recognize_command("not-a-windows-command"),
        CommandRecognition::Unknown
    );

    let wmic = classify_external("wmic").expect("WMIC remains an explicit entry");
    assert_eq!(
        (
            wmic.desktop.win10.before_21h1.status,
            wmic.desktop.win10.before_21h1.presence
        ),
        (VersionStatus::Supported, Presence::Inbox)
    );
    assert_eq!(
        (
            wmic.desktop.win10.from_21h1.status,
            wmic.desktop.win10.from_21h1.presence
        ),
        (VersionStatus::Deprecated, Presence::Inbox)
    );
    assert_eq!(
        (
            wmic.desktop.win11.before_24h2.status,
            wmic.desktop.win11.before_24h2.presence
        ),
        (VersionStatus::Deprecated, Presence::OptionalFeature)
    );
    assert_eq!(
        (
            wmic.desktop.win11.from_24h2.status,
            wmic.desktop.win11.from_24h2.presence
        ),
        (VersionStatus::Unsupported, Presence::Unavailable)
    );

    assert!(official_top_level_coverage().iter().any(|entry| {
        entry.source_name == "append"
            && entry.disposition == CatalogDisposition::UnsupportedOnDesktop
    }));
    assert!(official_top_level_coverage().iter().any(|entry| {
        entry.source_name == "adprep" && entry.disposition == CatalogDisposition::ServerOnly
    }));
    assert!(official_top_level_coverage().iter().any(|entry| {
        entry.source_name == "add" && entry.disposition == CatalogDisposition::SubcommandOnly
    }));
    assert!(official_top_level_coverage().iter().all(|entry| {
        !matches!(
            entry.disposition,
            CatalogDisposition::DesktopExternal
                | CatalogDisposition::OptionalDesktopFeature
                | CatalogDisposition::SeparateInstall
        ) || classify_external(entry.normalized_name).is_some()
    }));

    let mut builtin_alias_collision: ExternalCommand = *classify_external("ipconfig").unwrap();
    builtin_alias_collision.aliases = &["chdir"];
    assert!(validate_command_catalogs(&builtins(), &[builtin_alias_collision]).is_err());

    let mut contradictory_release: ExternalCommand = *classify_external("ipconfig").unwrap();
    contradictory_release.desktop.win11.from_24h2.status = VersionStatus::Unsupported;
    assert!(validate_command_catalogs(&builtins(), &[contradictory_release]).is_err());

    let mut unavailable_supported_release: ExternalCommand =
        *classify_external("ipconfig").unwrap();
    unavailable_supported_release
        .desktop
        .win10
        .from_21h1
        .presence = Presence::Unavailable;
    assert!(validate_command_catalogs(&builtins(), &[unavailable_supported_release]).is_err());
}

#[test]
fn official_az_snapshot_is_exact_and_accounts_for_every_source_name() {
    use sha2::{Digest, Sha256};

    let fixture = include_str!("../../../tests/fixtures/windows_cmd/windows_commands_az.tsv");
    let raw = include_str!("../../../tests/fixtures/windows_cmd/windows_commands_raw.md");
    let metadata = official_source_metadata().unwrap();
    assert_eq!(
        metadata.source_url,
        "https://raw.githubusercontent.com/MicrosoftDocs/windowsserverdocs/refs/heads/main/WindowsServerDocs/administration/windows-commands/windows-commands.md"
    );
    assert_eq!(metadata.fetched_on, "2026-08-29");
    assert_eq!(
        metadata.format,
        "official A-Z name<TAB>native family or source-name<TAB>disposition"
    );
    let raw_sha256 = format!("{:x}", Sha256::digest(raw.as_bytes()));
    assert_eq!(metadata.source_sha256, raw_sha256);
    assert_eq!(OFFICIAL_SOURCE_RAW_SHA256, raw_sha256);
    assert_eq!(
        OFFICIAL_SOURCE_RAW_SHA256,
        "b177c3014e3fa42294ed6fd5356d4bfa08e1a58a8b841e383dd5bcdc01837cc7"
    );
    assert_eq!(OFFICIAL_SOURCE_ENTRY_COUNT, 339);
    assert_eq!(
        OFFICIAL_SOURCE_FIXTURE_SHA256,
        "b5923b7a56ae1524664b117d972b05f253096568f892860afd234f3a3c3b19a1"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(fixture.as_bytes())),
        OFFICIAL_SOURCE_FIXTURE_SHA256
    );

    let coverage = official_top_level_coverage();
    assert_eq!(coverage.len(), OFFICIAL_SOURCE_ENTRY_COUNT);
    for expected in [
        "add alias",
        "begin backup",
        "compact vdisk",
        "Evntcmd",
        "freedisk",
        "gettype",
        "gpfixup",
        "gpt",
        "graftabl",
        "helpctr",
        "inactive",
        "ipxroute",
        "irftp",
        "jetpack",
        "ktmutil",
        "ktpass",
        "load metadata",
        "lpq",
        "lpr",
        "mmc",
        "net print",
        "powershell ise",
        "refsutil",
        "rwinsta",
        "tftp",
        "tpmtool",
        "uniqueid",
        "unlodctr",
        "wdsutil",
        "writer",
        "detach vdisk",
        "merge vdisk",
        "route ws2008",
        "winsat mem",
    ] {
        assert!(
            coverage.iter().any(|row| row.source_name == expected),
            "{expected}"
        );
    }
    assert_eq!(
        coverage
            .iter()
            .find(|row| row.source_name == "assign")
            .unwrap()
            .disposition,
        CatalogDisposition::SubcommandOnly
    );
    assert_eq!(
        coverage
            .iter()
            .find(|row| row.source_name == "manage bde")
            .unwrap()
            .normalized_name,
        "manage-bde"
    );
    for name in ["telnet", "tftp", "mount"] {
        let desktop = classify_external(name).unwrap().desktop;
        assert_eq!(
            desktop.win10.before_21h1.presence,
            Presence::OptionalFeature
        );
        assert_eq!(desktop.win10.from_21h1.presence, Presence::OptionalFeature);
        assert_eq!(
            desktop.win11.before_24h2.presence,
            Presence::OptionalFeature
        );
        assert_eq!(desktop.win11.from_24h2.presence, Presence::OptionalFeature);
    }
    let pwsh = classify_external("pwsh").unwrap().desktop;
    assert_eq!(pwsh.win10.before_21h1.presence, Presence::SeparateInstall);
    assert_eq!(pwsh.win10.from_21h1.presence, Presence::SeparateInstall);
    assert_eq!(pwsh.win11.before_24h2.presence, Presence::SeparateInstall);
    assert_eq!(pwsh.win11.from_24h2.presence, Presence::SeparateInstall);
}

#[test]
fn official_dispositions_must_match_release_scoped_manifest_metadata() {
    let coverage = official_top_level_coverage();
    let externals = external_commands();
    assert_eq!(validate_external_manifest_rows(coverage, externals), Ok(()));

    for (source_name, wrong_disposition) in [
        ("arp", CatalogDisposition::OptionalDesktopFeature),
        ("telnet", CatalogDisposition::DesktopExternal),
        ("pwsh", CatalogDisposition::OptionalDesktopFeature),
        ("wmic", CatalogDisposition::DesktopExternal),
    ] {
        let mut wrong_rows = coverage.to_vec();
        wrong_rows
            .iter_mut()
            .find(|row| row.source_name == source_name)
            .unwrap()
            .disposition = wrong_disposition;
        assert!(
            validate_external_manifest_rows(&wrong_rows, externals).is_err(),
            "{source_name} accepted {wrong_disposition:?}"
        );
    }

    let mut wrong_externals = externals.to_vec();
    wrong_externals
        .iter_mut()
        .find(|command| command.name == "arp")
        .unwrap()
        .desktop
        .win11
        .from_24h2
        .presence = Presence::OptionalFeature;
    assert!(validate_external_manifest_rows(coverage, &wrong_externals).is_err());
}

#[test]
fn official_builtin_and_subcommand_targets_are_exhaustively_resolved() {
    let coverage = official_top_level_coverage();
    assert_eq!(
        validate_external_manifest_rows(coverage, external_commands()),
        Ok(())
    );

    let mut typo = coverage.to_vec();
    typo.iter_mut()
        .find(|row| row.source_name == "active")
        .unwrap()
        .normalized_name = "diskpart-typo";
    let error = validate_external_manifest_rows(&typo, external_commands()).unwrap_err();
    assert!(error.contains("diskpart-typo"), "{error}");
}

#[test]
fn official_cmd_row_is_a_recognized_raw_executable_not_a_self_satisfying_builtin() {
    assert_eq!(recognize_command("cmd"), CommandRecognition::ExternalRaw);
    let coverage = official_top_level_coverage();
    let cmd = coverage
        .iter()
        .find(|row| row.source_name == "cmd")
        .unwrap();
    assert_eq!(cmd.normalized_name, "cmd");
    assert_eq!(cmd.disposition, CatalogDisposition::DesktopExternal);

    let mut invalid = coverage.to_vec();
    let mut fake_builtin = invalid[0];
    fake_builtin.source_name = "self-satisfying fake builtin";
    fake_builtin.normalized_name = "self-satisfying fake builtin";
    fake_builtin.disposition = CatalogDisposition::CmdBuiltin;
    invalid.push(fake_builtin);
    let error = validate_external_manifest_rows(&invalid, external_commands()).unwrap_err();
    assert!(error.contains("actual CMD builtin"), "{error}");
}

#[test]
fn external_modes_are_conservative_and_interpreter_targets_are_explicit() {
    for name in [
        "format",
        "diskpart",
        "diskshadow",
        "shutdown",
        "taskkill",
        "schtasks",
        "netsh",
        "powershell",
        "cscript",
        "ftp",
        "telnet",
        "pwsh",
    ] {
        let modes = classify_external(name).unwrap().modes;
        assert!(modes.contains(CommandModes::CONSERVATIVE_ANY), "{name}");
    }
    for name in ["ping", "pathping", "tracert"] {
        let modes = classify_external(name).unwrap().modes;
        assert!(modes.contains(CommandModes::QUERY.union(CommandModes::MACHINE)));
    }
    for name in ["reg", "bcdedit", "certutil", "wevtutil"] {
        let modes = classify_external(name).unwrap().modes;
        assert!(modes.contains(CommandModes::QUERY.union(CommandModes::MUTATION)));
    }
    let coverage = official_top_level_coverage();
    for (source, target) in [
        ("active", "diskpart"),
        ("add alias", "diskshadow"),
        ("rename", "ren"),
        ("reset", "diskshadow"),
        ("route ws2008", "route"),
    ] {
        assert_eq!(
            coverage
                .iter()
                .find(|row| row.source_name == source)
                .unwrap()
                .normalized_name,
            target
        );
    }
}

#[test]
fn every_recognized_raw_external_has_all_conservative_risk_modes() {
    for command in external_commands() {
        assert!(
            command.modes.contains(CommandModes::CONSERVATIVE_ANY),
            "{}",
            command.name
        );
    }
}

#[test]
fn raw_official_snapshot_header_and_names_match_the_checked_in_tsv() {
    let raw = include_str!("../../../tests/fixtures/windows_cmd/windows_commands_raw.md");
    let names: Vec<_> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("- ["))
        .filter_map(|value| value.split("](").next())
        .map(|value| value.trim_matches('`'))
        .collect();
    let fixture: Vec<_> =
        include_str!("../../../tests/fixtures/windows_cmd/windows_commands_az.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| line.split('\t').next().unwrap())
            .collect();
    assert_eq!(names, fixture);
}

#[test]
fn structured_display_filters_only_recognized_cmd_layouts() {
    let dir = include_str!("../../../tests/fixtures/windows_cmd/dir_default.txt");
    let set = include_str!("../../../tests/fixtures/windows_cmd/set_display.txt");
    let help = include_str!("../../../tests/fixtures/windows_cmd/help_assoc.txt");
    let assoc = include_str!("../../../tests/fixtures/windows_cmd/assoc_display.txt");
    let ftype = include_str!("../../../tests/fixtures/windows_cmd/ftype_display.txt");
    let grouped_dir = include_str!("../../../tests/fixtures/windows_cmd/dir_grouped_recursive.txt");
    let real_help = include_str!("../../../tests/fixtures/windows_cmd/help_assoc_real.txt");

    assert_eq!(
        filter_display("dir", "dir /a:-d /o:n C:\\work", dir),
        Some("[dir] C:\\work\r\nD .git\r\nF 42 README.md\r\n2 entries".to_owned())
    );
    assert_eq!(
        filter_display("set", "set RTK_", set),
        Some("[set] RTK_HOME=C:\\rtk; RTK_MODE=test".to_owned())
    );
    assert_eq!(
        filter_display("help", "help assoc", help),
        Some("[help] ASSOC [extension[=[fileType]]]\r\nDisplays or modifies file extension associations."
            .to_owned())
    );
    assert_eq!(
        filter_display("help", "help assoc", real_help),
        Some("[help] ASSOC [extension[=[fileType]]]\r\nDisplays or modifies file extension associations".to_owned())
    );
    assert_eq!(
        filter_display("assoc", "assoc", assoc),
        Some("[assoc] .rs=RustFile; .txt=txtfile".to_owned())
    );
    assert_eq!(
        filter_display("ftype", "ftype", ftype),
        Some("[ftype] RustFile=\"C:\\Tools\\rust.exe\" \"%1\"; txtfile=%SystemRoot%\\system32\\NOTEPAD.EXE %1"
            .to_owned())
    );

    assert!(!is_display_form("set", "set RTK_VALUE=mutates"));
    assert!(!is_display_form("set", "set /a 1+1"));
    assert!(!is_display_form("dir", "dir /b"));
    for source in [
        "dir /a",
        "dir /a:-d",
        "dir /c",
        "dir /-c",
        "dir /l",
        "dir /n",
        "dir /o:-n",
        "dir /s/a-d/o:n/t:w/4",
        "dir /^s/^a-d/^o:n",
    ] {
        assert!(
            is_display_form("dir", source),
            "supported detailed DIR layout should be eligible: {source}"
        );
    }
    for source in [
        "dir /p",
        "dir /q",
        "dir /x",
        "dir /w",
        "dir /d",
        "dir /r",
        "dir /s/p",
        "dir /s^/q",
        "dir /^X/s",
        "dir /z",
        "dir /apple",
        "dir /output",
        "dir /tomorrow",
        "dir /4ever",
    ] {
        assert!(
            !is_display_form("dir", source),
            "unsupported or alternate DIR layout must fail open before capture: {source}"
        );
    }
    for source in [
        "dir /s/b",
        "dir /b/s",
        "dir /a-d/b",
        "dir /A-D/B",
        "dir /^b",
        "dir /s/^b",
        "dir /^B/s",
        "dir /s^/b",
        "dir /S^/B",
    ] {
        assert!(
            !is_display_form("dir", source),
            "combined bare-format switch must remain native: {source}"
        );
    }
    assert!(
        is_display_form("dir", r#"dir "C:\reports /b archive""#),
        "a quoted path containing /b is not a DIR switch"
    );
    assert!(
        is_display_form("dir", r#"dir "C:\reports /p archive""#),
        "a quoted path containing /p is not a DIR switch"
    );
    assert_eq!(
        filter_display("assoc", "assoc", "Association missing"),
        None
    );
    assert_eq!(filter_display("help", "help assoc", "Aide inconnue"), None);
    assert_eq!(
        filter_display(
            "help",
            "help assoc",
            "Zeigt Dateizuordnungen\n\nASSOC [extension[=[fileType]]]\n\n  details"
        ),
        None
    );
    assert_eq!(
        filter_display(
            "set",
            "set RTK_",
            " RTK_LEADING= value with trailing space "
        ),
        Some("[set]  RTK_LEADING= value with trailing space ".to_owned())
    );
    assert_eq!(filter_display("unknown-adapter", "dir", dir), None);
    assert_eq!(
        filter_display("dir", "dir /s", grouped_dir),
        Some("[dir] C:\\work\r\nF 2,438 grouped.txt\r\n[dir] C:\\work\\nested\r\nF 7 deep.txt\r\n2 entries".to_owned())
    );
}

#[test]
fn catalog_identity_strategies_document_all_non_filtered_builtins_and_aliases() {
    for command in builtins() {
        match command.strategy.expect("validated catalog strategy") {
            AdapterStrategy::Identity { reason } => {
                assert!(
                    !reason.trim().is_empty(),
                    "{} needs an identity reason",
                    command.name
                );
                for alias in &command.aliases {
                    assert!(
                        command.matches(alias),
                        "{} alias {alias} must resolve",
                        command.name
                    );
                }
            }
            AdapterStrategy::Structured { adapter } => assert!(supports_adapter(adapter)),
        }
    }
}

#[test]
fn public_cmd_keeps_one_expression_raw_and_reconstructs_multiple_arguments() {
    assert_eq!(
        prepare_invocation(&[OsString::from("echo %CD% & dir /b")]).unwrap(),
        Invocation::Execute("echo %CD% & dir /b".to_owned())
    );
    assert_eq!(
        prepare_invocation(&[OsString::from("dir"), OsString::from("folder with spaces"),])
            .unwrap(),
        Invocation::Reconstructed("dir \"folder with spaces\"".to_owned())
    );
}

#[test]
fn public_cmd_normalizes_c_but_keeps_interactive_k_and_no_argument_sessions_native() {
    assert_eq!(
        prepare_invocation(&[
            OsString::from("/c"),
            OsString::from("dir"),
            OsString::from("/b"),
        ])
        .unwrap(),
        Invocation::Reconstructed("dir /b".to_owned())
    );
    assert_eq!(
        prepare_invocation(&[OsString::from("/K"), OsString::from("echo ready")]).unwrap(),
        Invocation::Passthrough(vec![OsString::from("/K"), OsString::from("echo ready")])
    );
    assert_eq!(
        prepare_invocation(&[]).unwrap(),
        Invocation::Passthrough(Vec::new())
    );
}

#[test]
fn nested_cmd_arguments_are_passed_as_argv_to_preserve_the_inner_parse() {
    let arguments = [
        OsString::from("cmd.exe"),
        OsString::from("/D"),
        OsString::from("/C"),
        OsString::from("echo"),
        OsString::from("nested quoted value"),
    ];

    assert_eq!(
        prepare_invocation(&arguments).unwrap(),
        Invocation::Passthrough(arguments.to_vec())
    );
}

#[test]
fn nested_cmd_metacharacter_arguments_fail_closed_before_the_second_parse() {
    for metacharacter in ["&", "|", ">"] {
        let result = prepare_invocation(&[
            OsString::from("cmd.exe"),
            OsString::from("/D"),
            OsString::from("/C"),
            OsString::from("echo"),
            OsString::from(metacharacter),
        ]);
        assert!(
            result.is_err(),
            "nested {metacharacter:?} must not reach CMD"
        );
    }
}

#[test]
fn absolute_cmd_host_paths_use_the_same_fail_closed_policy() {
    for executable in [
        r"C:\Windows\System32\cmd.exe",
        r"C:\Windows\System32\cmd.exe.",
        "C:\\Windows\\System32\\cmd.exe ",
    ] {
        let result = prepare_invocation(&[
            OsString::from(executable),
            OsString::from("/D"),
            OsString::from("/C"),
            OsString::from("echo"),
            OsString::from("&"),
        ]);
        assert!(
            result.is_err(),
            "absolute cmd.exe path spelling {executable:?} must not bypass syntax checks"
        );
    }
}

#[test]
fn hidden_transport_moves_long_operands_out_of_the_cmd_source() {
    let long_operand = "x".repeat(8_050);
    let Invocation::HiddenTransport {
        expression,
        environment,
    } = prepare_invocation(&[
        OsString::from("dir"),
        OsString::from("%RTK_LITERAL%"),
        OsString::from(long_operand.clone()),
    ])
    .unwrap()
    else {
        panic!("percent-bearing built-in arguments require hidden transport");
    };

    assert!(
        expression.len() < 2_000,
        "long operands must be transported, not interpolated"
    );
    assert!(environment
        .iter()
        .any(|(_, value)| value == &OsString::from(long_operand.as_str())));
}

#[test]
fn public_cmd_reconstruction_escapes_quotes_and_cmd_metacharacters_as_data() {
    assert_eq!(
        prepare_invocation(&[
            OsString::from("echo"),
            OsString::from(r#"safe" & echo injected > marker.txt"#),
        ])
        .unwrap(),
        Invocation::Reconstructed(r#"echo safe^"^ ^&^ echo^ injected^ ^>^ marker.txt"#.to_owned())
    );
}

#[test]
fn public_cmd_reconstruction_preserves_empty_and_bang_arguments() {
    assert_eq!(
        prepare_invocation(&[
            OsString::from("echo"),
            OsString::from(""),
            OsString::from("!RTK_CMD_UNSET!"),
        ])
        .unwrap(),
        Invocation::Reconstructed("echo ^\"^\" ^!RTK_CMD_UNSET^!".to_owned())
    );
}

#[test]
fn public_cmd_reconstruction_preserves_percent_and_hides_line_break_transport() {
    assert_eq!(
        prepare_invocation(&[OsString::from("echo"), OsString::from("100% complete"),]).unwrap(),
        Invocation::Reconstructed("echo 100^%^ complete".to_owned())
    );
    let Invocation::HiddenTransport {
        expression,
        environment,
    } = prepare_invocation(&[
        OsString::from("echo"),
        OsString::from("first line\r\nsecond line"),
    ])
    .unwrap()
    else {
        panic!("line breaks require the hidden transport");
    };
    for (key, _) in &environment {
        let key = key.to_string_lossy();
        assert!(expression.contains(&format!("set \"{key}=\"")));
    }
    assert!(environment
        .iter()
        .all(|(key, _)| !key.to_string_lossy().starts_with("RTK_CMD_ARG_")));
}

#[test]
fn public_cmd_reconstruction_keeps_literal_quotes_on_the_normal_route() {
    assert_eq!(
        prepare_invocation(&[
            OsString::from("definitely-not-installed.exe"),
            OsString::from(r#"{"a": 1}"#),
        ])
        .unwrap(),
        Invocation::Reconstructed(r#"definitely-not-installed.exe "{\"a\": 1}""#.to_owned())
    );
}

#[test]
fn hidden_line_break_transport_does_not_collide_with_user_environment() {
    let first_key = std::cell::RefCell::new(None);
    let result = super::orchestrator::prepare_line_break_transport_with_key_check(
        &["echo".to_owned(), "first\r\nsecond".to_owned()],
        |key| {
            let mut first_key = first_key.borrow_mut();
            if first_key.is_none() {
                *first_key = Some(key.to_owned());
                true
            } else {
                false
            }
        },
    );

    let Invocation::HiddenTransport { environment, .. } = result.unwrap() else {
        panic!("line breaks require hidden transport");
    };
    assert!(environment
        .iter()
        .all(|(key, _)| Some(key.to_string_lossy().as_ref()) != first_key.borrow().as_deref()));
}

#[test]
fn hidden_transport_rejects_unsafe_quoted_external_values_and_nested_cmd_syntax() {
    assert!(prepare_invocation(&[
        OsString::from("definitely-not-installed.exe"),
        OsString::from("first\r\n\"quoted\""),
    ])
    .is_err());
    let nested = [
        OsString::from("cmd.exe"),
        OsString::from("/D"),
        OsString::from("/C"),
        OsString::from("python"),
        OsString::from("first\r\nsecond"),
    ];
    assert!(prepare_invocation(&nested).is_err());
}

#[test]
fn multi_argument_dir_reaches_the_normal_terminal_rewrite_path() {
    let Invocation::Reconstructed(source) =
        prepare_invocation(&[OsString::from("dir"), OsString::from("folder with spaces")]).unwrap()
    else {
        panic!("multiple arguments must reconstruct one normal expression");
    };

    let rewritten = super::orchestrator::rewrite_expression_for_terminal(
        &source,
        Path::new(r"C:\Program Files\rtk.exe"),
        true,
    );
    assert!(rewritten.contains(SEGMENT_RUNNER));
}

#[test]
fn rewrite_fails_open_when_interpolated_rtk_path_can_expand_or_inject() {
    let source = "dir /o:n";
    for executable in [
        r"C:\%TEMP%\rtk.exe",
        r"C:\bang!name\rtk.exe",
        "C:\\quoted\" & echo injected\\rtk.exe",
        "C:\\line\r\nbreak\\rtk.exe",
    ] {
        assert_eq!(
            rewrite_expression(source, Path::new(executable)),
            source,
            "unsafe executable interpolation must fail open: {executable:?}"
        );
    }

    let quoted_safe =
        rewrite_expression(source, Path::new(r"C:\Program Files\rtk & tools\rtk.exe"));
    assert!(quoted_safe.starts_with(r#""C:\Program Files\rtk & tools\rtk.exe" __cmd-run"#));
}

#[test]
fn rewrite_enforces_the_full_8191_utf16_cmd_command_line_boundary() {
    let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
    let rtk = Path::new(r"C:\xrtk.exe");
    let at_limit = format!("dir {}", "x".repeat(4058));
    let over_limit = format!("dir {}", "x".repeat(4059));

    let rewritten =
        super::orchestrator::rewrite_expression_for_command_line(&at_limit, rtk, cmd, true);
    assert_ne!(
        rewritten, at_limit,
        "the fully encoded standard argument command line is exactly 8191 UTF-16 units"
    );
    assert_eq!(
        super::orchestrator::rewrite_expression_for_command_line(&over_limit, rtk, cmd, true),
        over_limit,
        "one more source byte expands to two hex units and must fail open"
    );

    let non_ascii = format!("dir {}", "界".repeat(1400));
    assert_eq!(
        super::orchestrator::rewrite_expression_for_command_line(&non_ascii, rtk, cmd, true),
        non_ascii,
        "UTF-8 source bytes become ASCII hex before the UTF-16 command-line check"
    );
}

#[test]
fn rewrite_keeps_identity_and_non_display_segments_in_the_parent_cmd_process() {
    let source = "  echo hello  && dir /b & cd .. & set RTK_TEST=value || type notes.txt";
    let rewritten = rewrite_expression(source, Path::new(r"C:\Program Files\rtk.exe"));

    assert_eq!(rewritten, source);
}

#[test]
fn rewrite_keeps_every_identity_builtin_and_alias_in_the_parent_cmd_process() {
    let executable = Path::new(r"C:\rtk.exe");
    for entry in builtins() {
        if matches!(entry.strategy, Some(AdapterStrategy::Identity { .. })) {
            for name in std::iter::once(entry.name).chain(entry.aliases.iter().copied()) {
                assert_eq!(rewrite_expression(name, executable), name, "{name}");
            }
        }
    }
    for source in ["echo exact text", "type binary.dat", "cls"] {
        assert_eq!(rewrite_expression(source, executable), source, "{source}");
    }
}

#[test]
fn rewrite_only_uses_cataloged_structured_adapters_for_terminal_displays() {
    let executable = Path::new(r"C:\rtk.exe");
    assert_eq!(
        super::orchestrator::rewrite_expression_for_terminal("dir /o:n", executable, false),
        "dir /o:n"
    );
    assert!(
        super::orchestrator::rewrite_expression_for_terminal("dir /o:n", executable, true)
            .contains(SEGMENT_RUNNER)
    );
    for source in ["assoc .rtk=RtkFile", "ftype RtkFile=cmd /c echo"] {
        assert_eq!(rewrite_expression(source, executable), source, "{source}");
    }
    for source in [
        "dir /s/b",
        "dir /b/s",
        "dir /a-d/b",
        "dir /A-D/B",
        "dir /^b",
        "dir /s/^b",
        "dir /^B/s",
        "dir /s^/b",
        "dir /S^/B",
    ] {
        assert_eq!(rewrite_expression(source, executable), source, "{source}");
    }
    assert!(
        rewrite_expression(r#"dir "C:\reports /b archive""#, executable).contains(SEGMENT_RUNNER),
        "quoted path text must not be parsed as a bare-format switch"
    );
}

#[test]
fn rewrite_preserves_at_prefix_and_fails_open_for_opaque_input() {
    let executable = Path::new(r"C:\rtk.exe");

    assert_eq!(rewrite_expression("@dir /b", executable), "@dir /b");
    assert_eq!(
        rewrite_expression("dir > listing.txt", executable),
        "dir > listing.txt"
    );
    assert_eq!(rewrite_expression("build.cmd", executable), "build.cmd");
    assert_eq!(
        rewrite_expression("set RTK_VALUE=kept & echo %RTK_VALUE%", executable),
        "set RTK_VALUE=kept & echo %RTK_VALUE%"
    );
}

#[test]
fn rewrite_fails_open_for_input_redirection_even_when_the_parser_is_not_opaque() {
    let source = "type < input.txt";
    let parsed = parse_expression(source);

    assert_eq!(parsed.opaque_reason, None);
    assert!(parsed
        .operators
        .iter()
        .any(|operator| operator.kind == OperatorKind::RedirectInput));
    assert_eq!(rewrite_expression(source, Path::new(r"C:\rtk.exe")), source);
}

#[test]
fn rewrite_runs_only_safe_structured_set_display_forms() {
    let executable = Path::new(r"C:\rtk.exe");

    assert_eq!(
        rewrite_expression("set RTK_PREFIX", executable),
        format!("C:\\rtk.exe {SEGMENT_RUNNER} --hex 7365742052544b5f505245464958")
    );
    assert_eq!(
        rewrite_expression("set RTK_PREFIX=value", executable),
        "set RTK_PREFIX=value"
    );
    assert_eq!(rewrite_expression("set /a 1+1", executable), "set /a 1+1");
}
