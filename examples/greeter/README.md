# Greeter

The WIT in `wit/world.wit` is the host application's interface contract. The host defines
the interfaces that plugins may implement, and plugin authors build against the host's
published WIT. Both the host and plugin point to this shared contract through `../wit`.

The plugin exports the greeter interface through the guest facade. The host admits that
component and invokes it through the generated typed role client.

From the repository root, build the plugin:

```bash
nix develop -c cargo build --manifest-path examples/greeter/plugin/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p greeter-host -- examples/greeter/plugin/target/wasm32-wasip2/release/greeter_plugin.wasm
```

The host prints `Hello, world!`.
