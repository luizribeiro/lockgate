# Audit

Your application runs one plugin on behalf of different users, and your own services
need to know who each call is for. That identity cannot come from the plugin: the plugin
must not be able to see it or lie about it. The host supplies it as call context so its
audit service can attribute each effect to the right user.

The `wit/` directory holds the shared contract: an audit interface plugins may import
and a tasks interface they may implement. The `plugin/` directory implements tasks and
reports progress through the audit import; the `host/` directory admits that component
once and invokes it for two users. `CallOrigin` is this example's call-context type, and
the audit handler reads it through `HostCtx::data()` before appending attributed messages
to shared storage.

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
