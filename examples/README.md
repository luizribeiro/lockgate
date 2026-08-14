# Examples

The greeter plugin declares and exports a small WIT interface through the guest facade.
The greeter host admits that component and invokes it through a hand-written public role.
Run these commands from the repository root:

```bash
nix develop -c cargo build --manifest-path examples/greeter-plugin/Cargo.toml --target wasm32-wasip2 --release
```

```bash
nix develop -c cargo run -p greeter-host -- examples/greeter-plugin/target/wasm32-wasip2/release/greeter_plugin.wasm
```
