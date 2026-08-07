# Lockgate

Lockgate is a small, auditable Rust library for capability-scoped WebAssembly component applications. Components keep ordinary generated WIT bindings; Lockgate discovers their actual interfaces from the binaries, links calls through isolated stores, and applies authority chosen by the embedding application.

This repository is evolving from a focused broker prototype into that library. The prototype remains runnable under `examples/demo`, so each extraction step can be checked against real components.

## The model

Lockgate deliberately keeps three kinds of identity separate:

- The application assigns a logical name such as `greeter` when adding bytes to a `Catalog`.
- Lockgate computes a SHA-256 digest over the exact artifact bytes.
- Package, interface, and function names come from the WIT embedded in the component.

Policy code therefore uses opaque catalog handles for component identity and verifies canonical WIT targets against the selected artifacts:

```rust,no_run
use lockgate::{Catalog, Policy, Runtime};

# fn example(
#     greeter_wasm: &[u8],
#     caller_wasm: &[u8],
#     dynamic_wasm: &[u8],
#     filereader_wasm: &[u8],
#     shared_dir: &std::path::Path,
# ) -> Result<(), Box<dyn std::error::Error>> {
let mut catalog = Catalog::new()?;
let greeter = catalog.add("greeter", greeter_wasm)?;
let caller = catalog.add("caller", caller_wasm)?;
let dynamic = catalog.add("dynamic", dynamic_wasm)?;
let filereader = catalog.add("filereader", filereader_wasm)?;

let policy = Policy::builder(&catalog)
    .link(caller, greeter)?
    .allow_lookup(
        dynamic,
        greeter,
        "demo:greeter/greeter@0.1.0#greet",
    )?
    .read_only_dir(filereader, shared_dir, "/shared")?
    .build();

let runtime = Runtime::builder(catalog, policy).build()?;
let values = runtime.call(
    caller,
    "demo:caller/runner@0.1.0#run",
    &[],
)?;
assert_eq!(values.len(), 1);
# Ok(())
# }
```

The public lifecycle is `Catalog` → `Policy` → `Runtime`. `RuntimeBuilder::build` performs deterministic, side-effect-free authority and structural-type validation before it creates any component store. Parsers and configuration-file formats remain outside Lockgate's core API.

## Repository layout

```text
src/
  catalog.rs             artifact identity and decoded component metadata
  policy.rs              immutable programmatic grants
  plan.rs                private provider resolution and preflight validation
  plugin.rs              shared structural type validation helpers
  runtime.rs             isolated stores and broker forwarding
  runtime/dynamic.rs     optional runtime-typed registry broker
wit/
  core.wit               optional dynamic registry owned by Lockgate
examples/demo/
  build.rs               builds and stages executable demo components
  src/main.rs            catalog, policy, runtime construction, calls, and narration
  src/artifacts.rs       staged component artifact lookup
  components/            four standalone Rust component crates
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

The root library has no build script and does not require `cargo-component` to compile or test. The example owns guest compilation and writes executable components to Cargo's `OUT_DIR`. Generated executable Wasm never lands beside component source.

## Two call paths

The example keeps two mechanisms visibly distinct:

- `caller` imports `demo:greeter/greeter@0.1.0` and calls `greeter::greet("world")` through normal `wit-bindgen` bindings. The host satisfies that typed import with a forwarding closure learned from the component binaries.
- `dynamic` imports `lockgate:core/registry`, computes a target string, and uses `lookup`/`invoke`. Handles, the small dynamic `Value` encoding, and runtime denial exist only on this opt-in path.

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

Greeter and caller share one immutable demo contract at `examples/demo/packages/demo-greeter-0.1.0.wasm`. Its editable source is `examples/demo/wit/packages/greeter/package.wit`. Rebuild the package explicitly when publishing a contract version:

```console
cd examples/demo
wkg wit build \
  --wit-dir wit/packages/greeter \
  --output packages/demo-greeter-0.1.0.wasm
```

Ordinary component builds consume that package and never regenerate it implicitly. This mirrors a registry release without requiring a private registry or duplicating dependency WIT under every component.

## Enforcement lifecycle

`Catalog` hashes the exact bytes it compiles, then uses `wit_component::decode` to read the real embedded world. Application names, artifact digests, and embedded WIT names remain distinct. Unsupported cross-store types are rejected before Wasmtime's dynamic type introspection.

`Policy` contains only catalog-owned handles. Grants automatically include their caller and provider; `.include(component)` adds a standalone component. An optional capability granted but not imported remains harmless.

`RuntimeBuilder::build` validates every included component before any store exists. For each typed component import it:

1. requires exactly one explicitly linked provider for the whole interface;
2. resolves every required function from that exact provider artifact;
3. structurally compares the caller's expected signature with the provider's actual signature;
4. rejects missing, ambiguous, incomplete, or mismatched links with named errors.

`RuntimeBuilder` accepts an optional observer before validation and instantiation, so initialization cannot create an observation gap. `Runtime` then gives each component its own Wasmtime `Store`, resource table, WASI context, bounded dynamic handle table, and fuel budget. It installs typed imports with `Linker::instance(...).func_new(...)`, begins with no preopens and network denied, and emits typed `Event` values instead of printing. The application decides whether and how to log broker crossings.

The runtime table lives outside every store. Wasmtime 47 requires forwarding callbacks and store data to satisfy `Send` bounds, so Lockgate uses `Arc<Mutex<...>>` with a separate lock per component. An identity-based RAII call stack rejects cycles and depth greater than eight before locking a callee, including mixed direct/dynamic chains. Fuel exhaustion becomes a trapped call, marks that component unhealthy, and leaves unrelated stores available.

Directory grants require an existing host directory and a normalized absolute POSIX guest path. The host path is canonicalized when policy is built, and the complete guest path is preserved. The demo grants only its staged `sandbox/shared` directory at `/shared`. Networking remains fail-closed because Wasmtime's current socket callback exposes resolved addresses rather than the requested hostname.

## Add a demo component

1. Copy a crate under `examples/demo/components/` and define its world in `wit/world.wit`.
2. Depend on the root `lockgate:core` WIT only if needed; use its import-free `plugin` world for ordinary components and `consumer` only for runtime-selected calls.
3. For typed calls, add the provider's versioned WIT package as a cargo-component target dependency and call the generated Rust binding normally.
4. Add the component in `examples/demo/src/main.rs`, retain the returned handles, and grant only its required authority.
5. Add the component ID to `IDS` in `examples/demo/build.rs`.
6. Run `cargo run -p lockgate-demo`.

## Acceptance experiments

- Inspect `examples/demo/components/caller/src/lib.rs`: it contains no dynamic `Value`, handle, or registry reference—only `greeter::greet`.
- Remove `.link(caller, greeter)` from the example policy. `RuntimeBuilder::build` refuses caller's real decoded import before any store exists.
- Give caller and provider different signatures. Runtime construction reports both structural types before guest code runs.
- Run the default demo. Dynamic shows an allowed exact lookup followed by an ungranted runtime `denied`; the host continues.
- Run `cargo test -p lockgate-demo`. The filesystem test calls filereader with `/etc/passwd` while only `/shared` is preopened and verifies that WASI denies it without making the component unhealthy.
- Run `cargo test -p lockgate` for identity, foreign handles/policies, directory validation, missing and ambiguous providers, complete structural signatures, unsupported types, fuel, health, depth, and cycle coverage.

## Current API substitutions and limits

The flake currently pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, and `wkg` 0.15.1. Rust dependencies pin Wasmtime 47.0.3 and `wit-component`/`wit-parser` 0.255.0.

- Signature enforcement uses `Component::component_type()` and Wasmtime's recursive `component::types::Type` equality instead of a handwritten WIT shape model.
- Wasmtime 47 links runtime-discovered imports with `Linker::instance(...).func_new(...)`; provider exports are resolved with `Instance::get_export_index` and called with `component::Func::call`.
- Direct provider cycles are allowed because forwarding closures resolve stores only when called; runtime identity and component handles detect re-entry before a store lock. A component that calls an import during its own instantiation can still fail if the provider store is not available yet.
- Resource handles and `error-context` cannot cross independent stores. Async functions, futures, and streams remain explicit non-goals.
- Wasmtime 47's public dynamic type conversion does not expose fixed-length lists and currently contains a `todo!()` for them. Lockgate rejects those from decoded WIT before runtime introspection rather than panicking.
- The optional dynamic registry intentionally supports only `bool`, `s32`, `u32`, `string`, and `list<string>`. Typed forwarding supports Wasmtime's broader structural value set.
- Host calls and dynamic grants identify an exact component handle and canonical WIT target. Unknown and existing-but-ungranted target strings are both denied without revealing catalog contents; repeated allowed lookups reuse one per-store handle.
