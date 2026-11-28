# rtk-engine

The RTK pipeline engine: it parses, resolves and validates `pipeline.toml`, evaluates rules, and runs the methods and chains those rules name. The `rtk` CLI links it for the hooks; the `rtk-proxy` daemon links it for `proxy.request` rules.

## User documentation

The pipeline reference lives in the user guide, [`docs/guide/pipelines/`](../docs/guide/pipelines/overview.md), published at [rtk-ai.app/guide](https://www.rtk-ai.app/guide/pipelines/overview):

| Page | Covers |
|---|---|
| [Pipelines](../docs/guide/pipelines/overview.md) | what the engine does, the files you might write, upgrading |
| [Writing a Pipeline](../docs/guide/pipelines/writing-pipelines.md) | file locations and load order, the rule tree, actions, routing, overrides |
| [Conditions and Judges](../docs/guide/pipelines/conditions.md) | every `when` key, the call context, classifiers |
| [Methods](../docs/guide/pipelines/methods.md) | built-in methods and chains, your own chains and filters |
| [Directives](../docs/guide/pipelines/directives.md) | text RTK puts in front of the agent |
| [Tasks and Upstreams](../docs/guide/pipelines/tasks.md) | delegation to a cheaper model |
| [Reference Pipelines](../docs/guide/pipelines/reference-pipelines.md) | the six built-ins as literal files |
| [Pipeline Commands](../docs/guide/pipelines/commands.md) | show, validate, explain, shadow, share, adopt |
| [Errors and Warnings](../docs/guide/pipelines/errors.md) | every diagnostic code, limits |
| [Plugins](../docs/guide/pipelines/plugins.md) | new methods and task kinds in Rust |

Worked pipelines: [`examples/pipelines/`](../examples/pipelines/README.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). `rtk methods list` prints every registered method with its contract.

## License

[Business Source License 1.1](LICENSE) — source-available, not OSI open source. Production use is free; offering the engine, or a derivative, to third parties as a paid or hosted product needs a licence from contact@rtk-ai.app. On 2030-09-09 this version converts to Apache-2.0. The `rtk` CLI's own source is Apache-2.0, and its Apache-2.0 edition is built without this crate — see [Editions and Licensing](../docs/guide/resources/editions.md).
