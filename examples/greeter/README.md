# Greeter

You have an application that you want third-party code to extend, but that code should
not become trusted application code. You publish a contract for the extension points,
plugin authors implement it, and Lockgate's kernel keeps their plugins contained while
your host calls them.

The `wit/` directory holds that shared WIT contract. The `plugin/` directory implements
its greeter interface through the guest facade, and the `host/` directory admits the
component and invokes it through the generated typed role client. Both sides build
against the contract in `wit/`.

From the repository root, build the plugin:

```bash
nix develop -c cargo build --manifest-path examples/greeter/plugin/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p greeter-host -- examples/greeter/plugin/target/wasm32-wasip2/release/greeter_plugin.wasm
```

The host prints `Hello, world!`.
