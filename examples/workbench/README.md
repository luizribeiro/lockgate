# Workbench

Your application has several extension points, and plugin authors implement only the
ones they care about. You do not require every plugin to fill every slot: the host asks
each admitted plugin which roles it serves, and `not implemented` is an ordinary typed
answer rather than a failure. A single plugin can serve several roles at once.

The host-owned `wit/` directory publishes the formatter, linter, and stats interfaces
plus the host's superset world; `host/` admits both compiled components and discovers
their roles at cast time. Each plugin directory declares its own subset world over a
vendored copy of that contract, exactly as an out-of-tree plugin author would. A host
test keeps those vendored copies byte-identical to the contract you publish. The host
uses per-plugin casts for feature detection, then generated `*_clients` fan-out methods
to invoke every implementer of each role in admission order.

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
tidy roles: formatter=implemented, linter=implemented, stats=not implemented
counter roles: formatter=not implemented, linter=not implemented, stats=implemented
formatter fan-out: tidy -> alpha beta gamma
linter fan-out: tidy -> line 2 has trailing whitespace
stats fan-out: counter -> 3
```
