# onto-cli

The `onto` command: a team ontology for dbt projects, grounded in
[OpenDataSuite](https://github.com/buchochelliq-labs/open-data-suite). It syncs the
team's facts (definitions, grains, standards, decisions and known issues, each with its
source), serves them to coding agents over MCP, and reviews the facts agents propose.

```sh
cargo install onto-cli   # installs the `onto` command
onto init --agents claude-code
onto sync
```

Prebuilt binaries are on the
[GitHub releases](https://github.com/buchochelliq-labs/open-data-suite-ontology/releases).
See the [repository](https://github.com/buchochelliq-labs/open-data-suite-ontology) for
documentation. Licensed under Apache-2.0.
