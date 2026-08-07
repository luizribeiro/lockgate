# Lockgate

Lockgate is a small, auditable Rust library for capability-scoped WebAssembly component applications. The embedding application owns its host/plugin WIT contracts and uses generated Wasmtime bindings. Lockgate discovers sibling interfaces from component binaries, resolves authorized providers, and forwards sibling calls between isolated stores without compiling those application-specific APIs into the library.

The design keeps API knowledge and implementation selection separate:

- Host-to-guest exports and guest-to-host imports use application-owned WIT and generated bindings.
- Sibling callers and providers compile against their shared WIT, but Lockgate learns that contract from their artifacts.
- Policy chooses which concrete component satisfies each sibling import and which host interfaces each component may import.
- Lockgate uses runtime component values only inside the sibling forwarding implementation. There is no guest-visible dynamic registry or public raw host-call API.

## The model

Lockgate keeps three kinds of identity separate:

- The application assigns a logical name such as `greeter` when adding bytes to a `Catalog`.
- Lockgate computes a SHA-256 digest over the exact artifact bytes.
- Package, interface, and function names come from the WIT embedded in the component.

The public lifecycle remains `Catalog` → `Policy` → `Runtime`:

```rust,no_run
use lockgate::{Catalog, Policy, Runtime};

# fn example(
#     greeter_wasm: &[u8],
#     caller_wasm: &[u8],
# ) -> Result<(), Box<dyn std::error::Error>> {
let mut catalog = Catalog::new()?;
let greeter = catalog.add("greeter", greeter_wasm)?;
let caller = catalog.add("caller", caller_wasm)?;

let policy = Policy::builder(&catalog)
    .link(caller, greeter)?
    .allow_host_import(caller, "myapp:host/services@1.0.0")?
    .build();

// The application normally follows this with `with_host`, `require_world`,
// and generated bindings as demonstrated under examples/demo.
let _builder = Runtime::builder(catalog, policy);
# Ok(())
# }
```

`RuntimeBuilder::with_host` supplies per-component application state and adds generated host imports to each component's linker. `require_world` runs generated world validation after all authorized imports are linked but before a store is created. `Runtime::with_instance` then provides guarded access to the store and instance so the application can construct its generated binding and make a typed call.

The runnable example contains the complete integration in `examples/demo/src/main.rs`.

## Call boundaries

The demo makes all three supported boundaries visible:

1. The host calls `demo:host/runnable.run` through a generated `RunnablePlugin` binding.
2. `caller` invokes the application host's `demo:host/services.log` through its generated guest binding.
3. `caller` invokes `demo:greeter/greeter.greet` through a normal generated sibling import. Lockgate satisfies that import using a provider selected by policy and structural types discovered from the two component artifacts.

Dynamic component selection is still possible: the application may choose a `ComponentId` at runtime and use the generated binding for the role that component was required to implement. Dynamic selection does not require dynamic typing.

If an application eventually needs guest-selected routing, it should define a domain-specific typed WIT service such as `render-with(provider, document)` rather than a universal string-and-value invocation protocol.

## WIT ownership

Lockgate owns no application WIT package. The demo application owns `demo:host`, which defines its host services and host-visible plugin roles:

```wit
package demo:host@0.1.0;

interface services {
  log: func(message: string);
}

interface runnable {
  run: func() -> result<string, string>;
}

world runnable-plugin {
  import services;
  export runnable;
}
```

The caller and filereader consume those contracts. The separately versioned `demo:greeter` package is shared only by the sibling caller and provider. Lockgate itself compiles against neither package.

Applications can define several plugin roles. A sibling-only provider does not need to implement a host-visible role, and unrelated plugin kinds do not need to share one artificial universal interface.

## Host integration

Application host traits are implemented directly on application state. `HasHost` projects that state out of Lockgate's `PluginStore` for generated `add_to_linker` functions:

```rust,ignore
impl bindings::myapp::host::services::Host for HostState {
    fn log(&mut self, message: String) {
        self.logs.push(message);
    }
}

let runtime = Runtime::builder(catalog, policy)
    .with_host(
        |_, component_name| HostState::new(component_name),
        |_, linker| {
            bindings::Plugin::add_to_linker::<_, HasHost<HostState>>(
                linker,
                PluginStore::host_mut,
            )?;
            Ok(())
        },
    )
    .require_world(plugin, |linker, component| {
        let pre = linker.instantiate_pre(component)?;
        bindings::PluginPre::new(pre)?;
        Ok(())
    })?
    .build()?;

let result = runtime.with_instance(plugin, |store, instance| {
    let bindings = bindings::Plugin::new(&mut *store, instance)?;
    Ok(bindings.myapp_plugin().call_run(&mut *store)?)
})?;
```

`PluginStore::resources_mut` is available when an application-owned generated host interface needs the component's resource table.

## Enforcement lifecycle

`Catalog` hashes and compiles the exact supplied bytes, then uses `wit_component::decode` and Wasmtime component types to expose their real imports, exports, and function signatures.

`Policy` contains catalog-owned handles. Grants automatically include their components; `.include(component)` adds a standalone component. Host imports and sibling links are distinct grants:

- `.allow_host_import(component, interface)` permits the embedding application to implement that exact imported interface.
- `.link(caller, provider)` permits the provider to satisfy matching sibling imports on the caller.
- Directory grants add narrowly scoped WASI preopens.

Before creating stores, runtime construction:

1. requires every custom import to have either a host grant or exactly one linked sibling provider;
2. resolves every function required from that sibling provider;
3. rejects async functions and values that cannot cross independent stores;
4. compares complete Wasmtime structural signatures;
5. configures generated application host bindings and validates every required application world.

Host-facing interfaces are not subjected to sibling cross-store restrictions. Their generated bindings and Wasmtime perform the relevant type checking.

Each component receives its own Wasmtime `Store`, application state, resource table, WASI context, and fuel budget. Stores begin with no preopens and networking denied. Sibling forwarding uses one mutex per component and an identity-based call stack that rejects cycles and depths greater than eight before locking a callee. A trapping call marks only that component unhealthy.

`RuntimeBuilder::with_observer` installs an application-owned observer before validation and instantiation. `Event::SiblingCall` reports the logical caller, selected provider, and canonical WIT target.

## Repository layout

```text
src/
  catalog.rs             artifact identity and decoded component metadata
  policy.rs              immutable host, sibling, and WASI grants
  plan.rs                private provider resolution and preflight validation
  plugin.rs              structural type inspection and cross-store validation
  runtime.rs             isolated stores, host hooks, and sibling forwarding
examples/demo/
  build.rs               builds and stages executable demo components
  src/main.rs            generated host bindings and the narrated lifecycle
  components/            three independent Rust component crates
  packages/              checked-in versioned sibling WIT package
  wit/packages/host/     application-owned host and plugin-role contracts
  sandbox/               demo filesystem input
```

Each guest component is its own tiny workspace, keeping its lockfile, target, and generated bindings independent from the host workspace.

## Run it

Install Nix with flakes and direnv, then allow the repository environment once:

```console
direnv allow
```

Run the narrated demo and all checks:

```console
cargo run -p lockgate-demo
cargo test --workspace
```

The root library has no build script and does not require `cargo-component` to compile or test. The demo owns guest compilation and stages generated Wasm under Cargo's `OUT_DIR`.

## Add a demo component

1. Choose or add an application-owned host-visible role under `examples/demo/wit/packages/host` if the host will call the component.
2. Define the component world under its own `wit/world.wit`, importing host services and sibling packages explicitly.
3. Use ordinary generated guest bindings for host and sibling calls.
4. Add the artifact to the catalog and grant each host import, sibling link, and WASI capability separately.
5. Configure the generated host binding and require its world before runtime construction.
6. Add the component ID to `IDS` in `examples/demo/build.rs`.

## Current limits

The flake pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, and `wkg` 0.15.1. Rust dependencies pin Wasmtime 47.0.3 and `wit-component`/`wit-parser` 0.255.0.

- Sibling forwarding supports synchronous functions whose values can move between independent stores. Resource handles, `error-context`, futures, and streams cannot cross that boundary.
- Wasmtime 47's public component type conversion does not expose fixed-length lists and contains an unimplemented conversion for them. Lockgate rejects those artifacts before runtime type introspection.
- Sibling provider cycles are linkable because forwarding closures resolve stores only when called; runtime guards reject re-entry. A component that calls an import during its own instantiation can still fail when its provider store is not available yet.
- The runtime is synchronous. Application-generated host bindings may use richer ordinary WIT types than sibling forwarding, but async execution is not currently configured.
- Networking remains fail-closed because Wasmtime's current socket policy callback exposes resolved addresses rather than the requested hostname.
