# Pipeline examples

Example pipeline files. For how pipeline files work, see
[Pipelines](../../docs/guide/pipelines/overview.md#three-files-you-might-write).
Copy one to `~/.config/rtk/pipeline.toml` (or drop several into
`~/.config/rtk/pipelines/`) and run `rtk pipeline validate`.

Writing nothing is also a configuration: with no pipeline file you are on
`rtk:default` — RTK exactly as it works today.

| File | What it does |
|---|---|
| `opt-in.toml` | turn on output compression (`rtk:posthook`) and the proxy (`rtk:proxy`) |
| `tune-one-thing.toml` | output compression plus delegated reads, with the oversize-read threshold raised by one `[override]` line |
| `delegate-reads-adoption.toml` | oversize reads route to a cheap model you bind |
| `classify-routing.toml` | a Jev classifier labels shell output; failing test runs route to a digest task |
| `laya-route.toml` | a local System-1 judge (Laya) gates routing of heavy shell output to Haiku/Sonnet digest workers — see its header for the prerequisite (local Laya, or hosted Jev + token) |
| `judge-before-tool.toml` | a judge decides before a read, command or fetch runs whether a cheaper model handles it; fail-open |
| `complex-session.toml` | a Jev classifier drives a custom plugin method plus built-in chains per output kind — needs a build with `--features example-plugins` |

The six built-in reference pipelines are compiled in — print any of them
with `rtk pipeline show rtk:default` / `rtk:posthook` / `rtk:proxy` /
`rtk:delegate-reads` / `rtk:verbatim-diffs` / `rtk:terse`.
