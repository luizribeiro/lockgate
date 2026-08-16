# Configuration

You want users to tune a plugin without giving the plugin access to files,
environment variables, or another host capability. Your application supplies a
JSON settings table during preparation, Lockgate validates it before admission,
and the plugin reads the retained value as its own Rust type. In this example,
the host chooses the visible `prefix` while omitting `ending`; serde applies the
plugin's `!` default when the guest deserializes the settings.

The `wit/` directory holds the formatter contract. The `plugin/` directory
declares one `Settings` struct, generates its schema, and calls
`Plugin::settings()` inside the formatter export. The `host/` directory supplies
`{"prefix":"Configured"}` to `prepare` and invokes the generated typed client.
The plugin requests no external authority: `NEEDS` remains `Needs::NOTHING`.

From the repository root, build the plugin:

```bash
nix develop -c cargo build --manifest-path examples/configuration/plugin/Cargo.toml --target wasm32-wasip2 --release
```

Then run the host:

```bash
nix develop -c cargo run -p configuration-host -- examples/configuration/plugin/target/wasm32-wasip2/release/configuration_plugin.wasm
```

The host prints:

```text
Configured: world!
```

If the host omits the whole settings table, `{}` is validated instead and the
required `prefix` property fails during `prepare`, before admission or any
application-facing plugin call. Misspelled keys fail there too because the
default settings policy closes the top-level object.
