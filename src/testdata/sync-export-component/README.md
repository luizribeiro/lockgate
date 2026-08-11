# Synchronous export test component

This fixture is a real `wit-bindgen` guest used by Lockgate's concurrent-store
regression test. To regenerate `../../../fixtures/sync-export-component.wasm`,
run:

```console
cargo build --manifest-path src/testdata/sync-export-component/Cargo.toml \
  --release --target wasm32-wasip2
cp src/testdata/sync-export-component/target/wasm32-wasip2/release/sync_export_component.wasm \
  fixtures/sync-export-component.wasm
```

If the target is unavailable but the Rust source is installed, add
`-Zbuild-std=std,panic_abort` to the build command.
