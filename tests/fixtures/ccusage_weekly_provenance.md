# ccusage weekly fixture provenance

These fixtures contain synthetic upstream test records, not user spend.

## Focused weekly snapshot

`ccusage_weekly_focused_v20_snapshot.json` is the unchanged JSON payload of an actual ccusage v20.0.26 focused Zcode renderer snapshot, with only its Insta header removed. The Sunday bucket starts use the same weekly/week shape as legacy ccusage. This is not a root-CLI capture or Claude billing fixture.

Source: https://github.com/ccusage/ccusage/blob/d9821088b98aa536c7a385aa1a4579d6fa02269b/rust/adapters/zcode/src/snapshots/ccusage_adapter_zcode__report__tests__focused_zcode_weekly_json.snap

## Root weekly synthetic fixture

`ccusage_weekly_root_v20_synthetic.json` is explicitly source-derived synthetic. The original daily record is moved into a weekly array and its period 2026-01-02 is changed to Monday 2025-12-29, following the root aggregator/serializer. All metrics, metadata and totals are unchanged. No live CLI was executed to produce it.

Original record: https://github.com/ccusage/ccusage/blob/d9821088b98aa536c7a385aa1a4579d6fa02269b/rust/crates/ccusage-adapter-all/src/snapshots/ccusage_adapter_all__tests__renders_multi_section_json_with_command_totals.snap

Aggregator: https://github.com/ccusage/ccusage/blob/d9821088b98aa536c7a385aa1a4579d6fa02269b/rust/crates/ccusage-adapter-all/src/loader.rs#L887-L906

Serializer: https://github.com/ccusage/ccusage/blob/d9821088b98aa536c7a385aa1a4579d6fa02269b/rust/crates/ccusage-adapter-all/src/report.rs#L133-L137

The reported totalTokens is 120 while the components sum to 130. Preserve the source's independent total, rather than reconstructing it.
