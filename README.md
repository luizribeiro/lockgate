# Capability-scoped component plugins

This is a small proof that isolated WebAssembly components can use ordinary typed imports across a host broker even when the host has no generated bindings for those interfaces. A separate plugin demonstrates the optional runtime-typed registry path.

There are two visibly different call mechanisms:

- `caller` imports `demo:greeter/greeter@0.1.0` and calls `greeter::greet("world")` through normal `wit-bindgen` bindings. At runtime the host satisfies that import with an untyped forwarding closure learned from the component binaries.
- `dynamic` imports `tangent:core/registry`, computes a target string, and uses `lookup`/`invoke`. Handles, `Value`, and the error taxonomy exist only on this explicitly dynamic path.

The host's only compile-time WIT knowledge is `tangent:core`. `demo:greeter` does not appear anywhere under `host/`.

## Host layout

- `main.rs` declares the modules and generated core binding, then starts the demo.
- `manifest.rs` defines capability policy and target matching.
- `plugin.rs` decodes component WIT, verifies manifests, and records structural types.
- `runtime.rs` owns isolated stores, WASI setup, direct forwarding, and the dynamic broker.
- `demo.rs` loads the five fixtures and prints the narrated transcript.
- `tests.rs` exercises the enforcement boundaries.

## Run it

Install Nix with flakes and direnv, then enter the repository once:

```console
direnv allow
```

After that, changing into this directory loads the pinned shell automatically. Run the complete demo from `host`:

```console
cd host
cargo run
```

The build script builds all five Rust guests with `cargo component` and places each ignored `.wasm` beside its `plugin.toml`. Run the enforcement tests with `cargo test`.

The greeter contract has one editable source at `wit/packages/greeter/package.wit`. Its checked-in, versioned distribution artifact is `packages/demo-greeter-0.1.0.wasm`; greeter, caller, and naughty all generate bindings from that 297-byte WIT package component. Rebuild it explicitly when publishing a contract version:

```console
wkg wit build \
  --wit-dir wit/packages/greeter \
  --output packages/demo-greeter-0.1.0.wasm
```

This local package stands in for a registry release, so ordinary guest builds never regenerate it implicitly.

## Worlds

`wit/core.wit` defines an import-free base and an opt-in consumer:

```wit
world plugin {}

world consumer {
  include plugin;
  import registry;
}
```

Greeter and filereader include `plugin` and have no registry import. Caller also includes `plugin`, then directly imports greeter. Only dynamic includes `consumer`.

The greeter interface is distributed once as `packages/demo-greeter-0.1.0.wasm`. Greeter exports that package's interface, while caller and naughty import it. This mirrors a registry dependency without requiring a registry service or duplicating WIT source under each consumer.

## Enforcement sequence

### Load

1. `wit_component::decode` reads the real world embedded in each component.
2. Imports must be a subset of declared capabilities. An imported registry requires `capabilities.registry = true`; filesystem and socket imports require their corresponding tables. Declaring an unused capability is harmless.
3. Every `provides` claim must be a real component export. Extra exports are not registered unless claimed.
4. The host records each claimed function's structural parameter and result shapes. Plugin-defined imports are recorded for the next phase rather than linked yet.

The preview1 adapter adds baseline WASI CLI, I/O, clock, and filesystem type imports even for simple guests. All plugins therefore declare an empty filesystem table; only filereader receives a path.

### Instantiate

Each plugin receives its own `Store`, resource table, WASI context, handle table, and fuel budget. The WASI context begins with no preopens and all network addresses denied.

For every plugin-defined direct import, the host:

1. Checks each `interface#function` against the caller's `invokes` globs.
2. Resolves its provider from the decoded registry.
3. Structurally compares the caller's expected signature with the provider's actual signature.
4. Creates the imported interface with `Linker::instance` and each function with `func_new`.

The forwarding callback receives `&[component::Val]`, calls the provider export on its separate store, and moves the returned values into the caller's result slice. Wasmtime lowers and lifts those values through the statically typed guest bindings at both ends.

`naughty` deliberately has a direct greeter import but no matching `invokes` entry. Its component loads successfully, then instantiation is refused with the full target name. This is distinct from dynamic's disallowed lookup, which returns `denied` at runtime without terminating dynamic or the host.

### Store borrowing and recursion

The plugin table lives outside every store. Forwarding closures capture it and obtain only the target runtime's lock, so the caller's already-borrowed store is never borrowed again.

Wasmtime 47 requires `func_new` closures to be `Send + Sync + 'static`, and `WasiView` requires store state to be `Send`. Consequently the reviewer's suggested `Rc<RefCell<PluginTable>>` cannot be used with this pinned API. The equivalent here is an `Arc<Mutex<PluginTable>>` containing one independent `Arc<Mutex<PluginRuntime>>` per store. A thread-local call stack rejects cycles and depth greater than eight before attempting to lock a callee, preventing deadlock.

Fuel traps are returned as `trapped` on the dynamic path or as a direct-call trap, and the callee is marked unhealthy.

## Manifest

```toml
id = "dynamic"
provides = ["demo:dynamic/runner@0.1.0"]
invokes = ["demo:greeter/greeter@0.1.0#greet"]

[capabilities]
registry = true

[capabilities.fs]
read = []
write = []

[capabilities.net]
hosts = []
```

Filesystem paths are relative to the repository root. Their basename becomes the guest mount point, so `./sandbox/shared` is mounted at `/shared`. Empty capability tables grant no resources.

Networking remains fail-closed in this five-plugin prototype. Socket imports require a non-empty host declaration, but no context enables network addresses. Wasmtime's current socket policy callback receives resolved addresses rather than requested hostnames, so hostname-aware allow-listing is left out rather than implemented imprecisely.

## Add a plugin

1. Copy a directory under `plugins/` and define its exports in `wit/world.wit`.
2. Include `tangent:core/plugin@0.1.0`. Include `consumer` instead only if the target truly is runtime-selected.
3. For direct calls, add the versioned WIT package component under `package.metadata.component.target.dependencies`, import its interface in the world, and call its generated Rust bindings normally.
4. Make `provides`, `invokes`, and capabilities describe the component's authority.
5. Add the ID to `IDS` in `host/src/demo.rs` and to the list in `host/build.rs`.
6. Run `cargo run` in `host`.

The dynamic registry encoding deliberately supports only `bool`, `s32`, `u32`, `string`, and `list<string>`. Direct forwarding is not limited to this encoding; it passes component values after structural signature matching.

## Acceptance experiments

- Inspect `plugins/caller/src/lib.rs`: it contains no `Value`, handle, or registry reference, only `greeter::greet`.
- Run `cargo test`. The direct-import test gives caller and provider different structural signatures and verifies that instantiation resolution reports both expected and provided types. Legitimate local guests cannot drift accidentally because they compile against the same immutable package; independently distributed or dishonest components are still checked from their decoded binaries.
- Remove caller's `invokes` entry. Caller is refused at instantiation with the target name.
- Run the default demo. Naughty shows instantiation-time direct-import denial; dynamic shows an allowed runtime call followed by a runtime `denied`; the host continues.
- The test suite also covers unused capability declarations, missing registry capability, fuel exhaustion/unhealthy state, depth, and cycle guards.
- Change filereader's guest path away from `/shared`. WASI returns an error because no other directory is preopened.

## Current API substitutions

The flake currently pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, and `wkg` 0.15.1. Rust dependencies pin Wasmtime 47.0.3 and `wit-component`/`wit-parser` 0.255.0.

- Current WIT rejects the original recursive `value` case `list(list<value>)`; the unused nested case is represented as `list<string>`. Keyword cases require `%s32` and `%u32` escapes.
- Local cargo-component worlds use `[package.metadata.component.target]` and `[package.metadata.component.target.dependencies]`.
- `wkg wit build` packages editable WIT as a component binary; cargo-component 0.21.1 accepts that binary as a target dependency and generates normal Rust bindings from it.
- Wasmtime 47 links generated imports with `HasSelf`, exposes Preview 2 through `wasmtime_wasi::p2::add_to_linker_sync`, and accesses context state through `WasiCtxView`.
- Dynamic direct imports use `Linker::instance(...).func_new(...)`. Nested provider exports are resolved with `Instance::get_export_index`, called with `component::Func::call`, and sized through `Func::ty().results()`.
- `Arc<Mutex<...>>` replaces the proposed `Rc<RefCell<...>>` because of Wasmtime 47's required callback and store bounds.
