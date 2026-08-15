# Audit

The WIT in `wit/world.wit` is the host application's interface contract. The host defines
the audit interface that plugins may import and the tasks interface they may implement.
Both the host and plugin point to this shared contract through `../wit`.

The plugin reports task progress through the host's audit import and explicitly declares
`Needs::NOTHING`: application imports are the host's own surface, not requested host
capabilities. The host admits that component once and invokes it twice on behalf of two
different users.

`CallOrigin` is the per-call value that tunnels past the plugin, from the host's call site
to the host's import handlers. It is absent from the WIT and the plugin's arguments, so the
guest cannot observe or forge it. The audit handler reads this call data through
`HostCtx::data()` and appends attributed messages to storage shared by the cloned imports.

From the repository root, build the plugin:

```bash
nix develop -c cargo build --manifest-path examples/audit/plugin/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p audit-host -- examples/audit/plugin/target/wasm32-wasip2/release/audit_plugin.wasm
```

The host prints:

```text
alice: started index
alice: finished index
bob: started backup
bob: finished backup
```
