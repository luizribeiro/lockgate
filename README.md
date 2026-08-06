# Lockgate

Lockgate is a small, auditable Rust library for capability-scoped WebAssembly component applications. Components keep ordinary generated WIT bindings; Lockgate discovers their actual interfaces from the binaries, links calls through isolated stores, and applies authority chosen by the embedding application.

This repository is evolving from a focused broker prototype into that library. The prototype remains runnable under `examples/demo`, so each extraction step can be checked against real components.

## The model

Lockgate deliberately keeps three kinds of identity separate:

- The application assigns a logical name such as `greeter` when adding bytes to a `Catalog`.
- Lockgate computes a SHA-256 digest over the exact artifact bytes.
- Package, interface, and function names come from the WIT embedded in the component.

Policy code therefore uses opaque catalog handles rather than names copied out of Wasm files:

```rust,no_run
use lockgate::{Catalog, Policy};

# fn example(
#     greeter_wasm: &[u8],
#     caller_wasm: &[u8],
#     dynamic_wasm: &[u8],
# ) -> Result<(), Box<dyn std::error::Error>> {
let mut catalog = Catalog::new()?;
let greeter = catalog.add("greeter", greeter_wasm)?;
let caller = catalog.add("caller", caller_wasm)?;
let dynamic = catalog.add("dynamic", dynamic_wasm)?;
let greet = catalog.export(
    greeter,
    "demo:greeter/greeter@0.1.0#greet",
)?;

let policy = Policy::builder(&catalog)
    .link(caller, greeter)?
    .allow_lookup(dynamic, greet)?
    .read_only_dir(caller, "./shared", "/shared")?
    .build();

assert_eq!(policy.links().len(), 1);
# Ok(())
# }
```

`Catalog` and the immutable `Policy` builder are public today. Runtime construction is still being extracted from the prototype internals. The demo's TOML manifests are a temporary adapter and fixture format; parsers and configuration-file formats are intentionally not part of Lockgate's core API.

## Repository layout

```text
src/
  catalog.rs             artifact identity and decoded component metadata
  policy.rs              immutable programmatic grants
  manifest.rs            temporary demo TOML adapter
  plugin.rs              prototype loading and structural type discovery
  runtime.rs             isolated stores and broker forwarding
  demo.rs                temporary narrated-demo adapter
wit/
  core.wit               optional dynamic registry owned by Lockgate
examples/demo/
  build.rs               builds and stages the demo components
  components/            five standalone Rust component crates
  packages/              checked-in versioned demo WIT package
  sandbox/               demo filesystem input
  wit/                    editable demo-owned shared contracts
```

Each guest component is its own tiny workspace. This keeps its lockfile, target, and generated bindings independent from the host workspace.

## Run it

Install Nix with flakes and direnv, then allow the repository environment once:

```console
direnv allow
```

Run the narrated prototype:

```console
cargo run -p lockgate-demo
```

Run the library and workspace checks:

```console
cargo test -p lockgate
cargo test --workspace
```

The root library has no build script and does not require `cargo-component` to compile or test. The example owns guest compilation. Its build script writes executable components and adjacent manifest copies to Cargo's `OUT_DIR`, then points the demo at that staged tree. Generated executable Wasm never lands beside component source.

## Two call paths

The example keeps two mechanisms visibly distinct:

- `caller` imports `demo:greeter/greeter@0.1.0` and calls `greeter::greet("world")` through normal `wit-bindgen` bindings. The host satisfies that typed import with a forwarding closure learned from the component binaries.
- `dynamic` imports `tangent:core/registry`, computes a target string, and uses `lookup`/`invoke`. Handles, the small dynamic `Value` encoding, and runtime denial exist only on this opt-in path.

The host has no generated binding for `demo:greeter`. It uses the signature decoded from the caller and provider components, compares their complete Wasmtime structural types, and forwards `component::Val` values between their independently owned stores.

## WIT ownership

`wit/core.wit` is the one library-owned contract. It defines an import-free base world and an opt-in dynamic consumer:

```wit
world plugin {}

world consumer {
  include plugin;
  import registry;
}
```

Greeter, caller, and naughty share one immutable demo contract at `examples/demo/packages/demo-greeter-0.1.0.wasm`. Its editable source is `examples/demo/wit/packages/greeter/package.wit`. Rebuild the package explicitly when publishing a contract version:

```console
cd examples/demo
wkg wit build \
  --wit-dir wit/packages/greeter \
  --output packages/demo-greeter-0.1.0.wasm
```

Ordinary component builds consume that package and never regenerate it implicitly. This mirrors a registry release without requiring a private registry or duplicating dependency WIT under every component.

## Prototype enforcement

At load, `wit_component::decode` reads the real embedded world. Imported capabilities must be a subset of declared authority; declaring an unused capability remains harmless. Every claimed export must exist, and plugin-defined interfaces are checked for supported synchronous cross-store types.

At instantiation, each component receives its own Wasmtime `Store`, resource table, WASI context, handle table, and fuel budget. The host begins with no preopened directories and network access denied. For a typed component import it:

1. checks policy for each `interface#function`;
2. resolves a provider from decoded exports;
3. structurally compares the caller's expected signature with the provider's actual signature;
4. installs a `Linker::instance(...).func_new(...)` forwarding closure.

The plugin table lives outside every store. Wasmtime 47 requires forwarding callbacks and store data to satisfy `Send` bounds, so the prototype uses `Arc<Mutex<...>>` with a separate lock per runtime. A thread-local call stack rejects cycles and depth greater than eight before locking a callee. Fuel exhaustion becomes a trapped call and marks the callee unhealthy without terminating the host.

Filesystem paths in the demo manifests are relative to the staged example root. `./sandbox/shared` is mounted at `/shared`; nothing else is preopened. Networking remains fail-closed because Wasmtime's current socket callback exposes resolved addresses rather than the requested hostname.

## Add a demo component

1. Copy a crate under `examples/demo/components/` and define its world in `wit/world.wit`.
2. Depend on the root `tangent:core` WIT only if needed; use its import-free `plugin` world for ordinary components and `consumer` only for runtime-selected calls.
3. For typed calls, add the provider's versioned WIT package as a cargo-component target dependency and call the generated Rust binding normally.
4. Add the fixture manifest and application policy grants.
5. Add the component ID to `IDS` in `examples/demo/build.rs` and `src/demo.rs` while the transitional adapter remains.
6. Run `cargo run -p lockgate-demo`.

## Acceptance experiments

- Inspect `examples/demo/components/caller/src/lib.rs`: it contains no dynamic `Value`, handle, or registry reference—only `greeter::greet`.
- Remove caller's `invokes` entry. The current adapter refuses caller at instantiation with the complete target name.
- Give caller and provider different signatures. Instantiation reports both structural types before greeter code runs.
- Run the default demo. Naughty shows instantiation-time typed-import denial; dynamic shows an allowed runtime call followed by a runtime `denied`; the host continues.
- Change filereader's guest path away from `/shared`. WASI returns an error because no other directory is preopened.
- Run `cargo test -p lockgate` for capability-subset, signature, unsupported-type, foreign-handle, policy, fuel, depth, and cycle coverage.

## Current API substitutions and limits

The flake currently pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, and `wkg` 0.15.1. Rust dependencies pin Wasmtime 47.0.3 and `wit-component`/`wit-parser` 0.255.0.

- Signature enforcement uses `Component::component_type()` and Wasmtime's recursive `component::types::Type` equality instead of a handwritten WIT shape model.
- Wasmtime 47 links runtime-discovered imports with `Linker::instance(...).func_new(...)`; provider exports are resolved with `Instance::get_export_index` and called with `component::Func::call`.
- Resource handles and `error-context` cannot cross independent stores. Async functions, futures, and streams remain explicit non-goals.
- Wasmtime 47's public dynamic type conversion does not expose fixed-length lists and currently contains a `todo!()` for them. Lockgate rejects those from decoded WIT before runtime introspection rather than panicking.
- The optional dynamic registry intentionally supports only `bool`, `s32`, `u32`, `string`, and `list<string>`. Typed forwarding supports Wasmtime's broader structural value set.
