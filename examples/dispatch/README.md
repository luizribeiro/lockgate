# Dispatch

Your application lets plugins hand work back to it, such as a report to deliver or a
sync to start, while each plugin call stays snappy and bounded. Once handed over, that
work belongs to the host: it can outlive the call that requested it, and any failure
goes to your application's error sink rather than back to the plugin. Each detached job
is a host future under host ownership; Lockgate enforces a per-plugin count quota and
drains the jobs when the host shuts down. The plugin only ever asked.

The `wit/` directory holds the shared contract: a dispatch interface plugins may import
and a tasks interface they may implement. The `plugin/` directory asks the host to
deliver reports and returns immediately; the `host/` directory detaches the slow
delivery futures with unit call context, reports one failure through its error sink,
and drops the host through `spawn_blocking` so shutdown remains safe in an async
embedder.

From the repository root, build the plugin:

```bash
nix develop -c cargo build --manifest-path examples/dispatch/plugin/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p dispatch-host -- examples/dispatch/plugin/target/wasm32-wasip2/release/dispatch_plugin.wasm
```

The host prints:

```text
plugin calls returned: queued index; queued backup
delivered: index report
delivered: backup report
detached job failed: plugin=dispatch job=3 error=could not deliver undeliverable report
```
