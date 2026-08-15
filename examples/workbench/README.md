# Workbench

Your application has several extension points, and plugin authors implement only the
ones they care about. You do not require every plugin to fill every slot: the host asks
each admitted plugin which roles it serves, and `not implemented` is an ordinary typed
answer rather than a failure. A single plugin can serve several roles at once.

The `wit/` directory defines the host world's superset vocabulary of formatter, linter,
and stats roles, plus a subset world for each plugin. The `plugins/tidy/` directory
targets the formatter-and-linter world, while `plugins/counter/` targets the stats world;
the `host/` directory admits both components and tries every plugin-role pairing. Role
discovery happens at cast time against each compiled component, before any plugin call.

From the repository root, build both plugins:

```bash
nix develop -c cargo build --manifest-path examples/workbench/plugins/tidy/Cargo.toml --target wasm32-wasip2 --release
nix develop -c cargo build --manifest-path examples/workbench/plugins/counter/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p workbench-host -- examples/workbench/plugins/tidy/target/wasm32-wasip2/release/tidy_plugin.wasm examples/workbench/plugins/counter/target/wasm32-wasip2/release/counter_plugin.wasm
```

The host prints:

```text
tidy formatter: alpha beta gamma
tidy linter: line 2 has trailing whitespace
tidy stats: not implemented
counter formatter: not implemented
counter linter: not implemented
counter stats: 3
```
