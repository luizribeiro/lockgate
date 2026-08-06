# Capability-scoped component plugins

This is a deliberately small proof that isolated WebAssembly components can call one another through a host broker whose target interfaces are unknown when the host is compiled.

The host has generated bindings for one interface only: `tangent:core/registry`. At load time it decodes each component's embedded WIT, checks the manifest against that WIT, and builds a registry of untyped Wasmtime component exports. `demo:greeter` never appears in the host's WIT or Rust types.

## Run it

Install Nix with flakes and direnv, then enter the repository once:

```console
direnv allow
```

After that, changing into the directory loads the pinned shell automatically. Run the complete demo from `host`:

```console
cd host
cargo run
```

The host build script builds all four Rust guests with `cargo component`, places each `.wasm` next to its `plugin.toml`, and then runs the narrated demo. Generated bindings, components, and targets are intentionally ignored by Git.

To run the enforcement tests:

```console
cargo test
```

## What is enforced

Loading and execution have separate boundaries:

1. `wit_component::decode` reads the actual imported and exported interfaces from each component.
2. A manifest `provides` entry must be a real export, and every exported interface must be declared. Unknown non-WASI imports fail loading with their full interface name.
3. Each plugin gets a separate `Store`, resource table, WASI context, handle table, and fuel budget.
4. The WASI context starts closed. Only manifest filesystem paths are preopened; read entries receive read-only permissions. Wasmtime denies network addresses by default.
5. `lookup` first checks the caller's `invokes` globs, then consults the decoded registry. Handles live in the caller's store and cannot be shared.
6. `invoke` checks arity and every dynamic `value` case against the decoded callee parameter types before running guest code. It then calls the callee's export through `component::Val` on the callee's store.
7. A thread-local call stack caps depth at eight and rejects cycles. Fuel traps are converted to `trapped`, and the callee is marked unhealthy without taking down its caller.

All WASI interfaces are linked consistently. A missing capability is represented by a closed `WasiCtx`, rather than a per-interface trap stub. This is necessary because the `cargo-component` preview1 adapter embeds baseline CLI, I/O, clock, and filesystem type imports even in guests that do not perform I/O. The loader still rejects non-WASI imports that are not the registry, and requires the matching `capabilities.fs` or `capabilities.net` table for filesystem or socket imports.

Network host lists are parsed and socket imports without a non-empty declaration are refused, but this four-plugin prototype does not enable networking. The default WASI context therefore remains fail-closed for every address. Hostname-aware networking would require a separate demo because Wasmtime's current policy callback receives resolved socket addresses, not the requested hostname.

## Manifest

Each plugin has a `plugin.toml` beside its component:

```toml
id = "greeter"
provides = ["demo:greeter/greeter@0.1.0"]
invokes = ["demo:other/api@0.1.0#*"]

[capabilities.fs]
read = ["./sandbox/shared"]
write = []

[capabilities.net]
hosts = []
```

Filesystem source paths are relative to the repository root. Their basename is mounted at the guest root, so `./sandbox/shared` becomes `/shared`. An empty table declares the interface while granting no paths or hosts.

## Add a plugin

1. Copy one of the small directories under `plugins/` and give it a unique package and interface in `wit/world.wit`.
2. Import `tangent:core/registry@0.1.0` and add the local dependency under `[package.metadata.component.target.dependencies]` in `Cargo.toml`.
3. Implement the generated guest trait in `src/lib.rs` and describe the real exports, calls, and capabilities in `plugin.toml`.
4. Add the ID to `IDS` in `host/src/main.rs` and to the list in `host/build.rs`.
5. Run `cargo run` in `host`. A manifest/WIT disagreement is reported before instantiation.

The dynamic broker currently supports `bool`, `s32`, `u32`, `string`, and `list<string>`. It reports other decoded WIT types as unsupported instead of guessing an encoding.

## Acceptance experiments

The default transcript demonstrates an unknown-at-compile-time greeter call, a read-only preopen, a denied lookup, and a pre-call type mismatch. The tests cover an undeclared import, fuel exhaustion/unhealthy state, depth, and cycle guards.

The mutation checks can be tried directly; the build script notices guest WIT, source, and manifest changes:

- Remove `greet` from greeter's WIT. Greeter is refused because its `provides` claim is now false; caller subsequently gets `not-found`, and the remaining calls continue.
- Change `greet` to accept `s32` and update its Rust implementation to accept `i32`. Caller still sends `string`, so the broker reports `type-mismatch` before greeter runs.
- Delete caller's `invokes` line. Serde defaults it to an empty list and lookup reports `denied`.
- Add a custom imported interface to a guest and reference one of its functions so the linker retains it. Loading is refused and names the undeclared interface.
- Change filereader's guest path away from `/shared`. WASI returns an error because no other directory is preopened.
- Run `cargo test fuel_trap_marks_callee_unhealthy` for an infinite component function, or `cargo test rejects_depth_limit_and_cycles` for the recursion guards.

## Current API substitutions

This repository pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, Wasmtime 47.0.3, and `wit-component`/`wit-parser` 0.255.0.

- Current WIT rejects the proposed recursive `value` case `list(list<value>)` with “type value depends on itself.” The unused nested case is represented as `list<string>` instead. WIT keywords used as variant cases also require `%s32` and `%u32` escapes.
- A local cargo-component world now uses `[package.metadata.component.target] path = "wit"` and puts WIT dependencies in `[package.metadata.component.target.dependencies]`.
- Wasmtime 47 links the registry with `HasSelf`, exposes WASI Preview 2 through `wasmtime_wasi::p2::add_to_linker_sync`, and accesses WASI state through `WasiCtxView`.
- Dynamic nested exports are resolved with `Instance::get_export_index` and called with `component::Func::call`; result arity comes from `Func::ty().results()`.

These choices keep the broker itself synchronous and free of generated knowledge about plugin-defined interfaces.
