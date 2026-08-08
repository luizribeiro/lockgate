# Lockgate

Lockgate is a small, auditable Rust library for capability-scoped WebAssembly component applications. The embedding application owns its host/plugin WIT contracts and uses generated Wasmtime bindings. Lockgate discovers sibling interfaces from component binaries, resolves authorized providers, and forwards sibling calls between isolated stores without compiling those application-specific APIs into the library.

The design keeps API knowledge and implementation selection separate:

- Host-to-guest exports and guest-to-host imports use application-owned WIT and generated bindings.
- Sibling callers and providers compile against their shared WIT, but Lockgate learns that contract from their artifacts.
- Application grants choose which concrete component satisfies each sibling import and which host interfaces each component may import.
- Lockgate uses runtime component values only inside the sibling forwarding implementation. There is no guest-visible dynamic registry or public raw host-call API.

## The model

Lockgate keeps three kinds of identity separate:

- The application assigns a logical name such as `greeter` when adding bytes to its catalog.
- Package, interface, and function names come from the WIT embedded in the component.

The public lifecycle is generated bindings → `Application` → `Runtime`:

```rust,ignore
use lockgate::{Application, HostContext};

mod bindings {
    lockgate::bindings! {
        path: "wit",
        worlds: {
            GreeterPlugin: "greeter-plugin",
            RunnablePlugin: "runnable-plugin",
            FileReaderPlugin: "file-reader-plugin",
        },
    }
}

impl bindings::myapp::host::services::Host for HostContext<()> {
    fn log(&mut self, message: String) {
        println!("{}: {message}", self.component_name());
    }
}

let mut app = Application::new(())?;
let greeter = app.add::<bindings::GreeterPlugin>("greeter", greeter_wasm)?;
let caller = app.add::<bindings::RunnablePlugin>("caller", caller_wasm)?;

let runtime = app
    .link(caller, greeter)?
    .allow_host_import(caller, "myapp:host/services@1.0.0")?
    .run()?;

let result = runtime.component(caller).run()?;
```

`lockgate::bindings!` parses all listed application worlds together. It generates every imported host interface once, remaps each Wasmtime world to those shared bindings, and emits export contracts plus canonical host-interface installers. The application therefore implements `myapp:host/services@1.0.0` once even when several plugin roles import it.

`Application::add::<Role>` compares each requested generated export contract with metadata decoded from the artifact. A single role returns one typed component handle; a tuple of roles returns a matching tuple of handles after every role validates atomically. It also retains host-interface installers specialized for the application's state type. Even sibling-only providers use a narrow export-only role. Runtime construction installs only explicitly granted host interfaces and prepares every complete linker before creating stores.

`Runtime::component` turns a typed handle into a lightweight generated client. Each binding role represents one exported interface, so its WIT functions become ordinary Rust methods directly on that client while Lockgate keeps Wasmtime stores, binding construction, fuel, locking, and trap health internal. A WIT `result<T, E>` remains inside the outer `Result<_, RuntimeError>`, so domain errors do not mark a component unhealthy.

The generated binding implementation exists only when `HostContext<S>` implements every host trait imported by that role. A missing host implementation is therefore a compile-time error at `Application::add`, while artifact admission and grant validation remain runtime checks over the supplied bytes and grants.

The runnable example contains the complete integration in `examples/demo/src/main.rs`.

## Call boundaries

The demo makes all three supported boundaries visible:

1. The host calls `demo:host/runnable.run` through a generated `RunnablePlugin` binding.
2. `caller` invokes the application host's `demo:host/services.log` through its generated guest binding.
3. `caller` invokes `demo:greeter/greeter.greet` through a normal generated sibling import. Lockgate satisfies that import using an explicitly linked provider and structural types discovered from the two component artifacts.

Dynamic component selection is still possible: the application may choose among `Component<RunnablePlugin>` handles at runtime. Every call still goes through a generated typed client, so selecting an artifact dynamically does not erase its interface authority.

An artifact can implement several independent roles without being instantiated more than once. Add it with each narrow role needed by the application:

```rust,ignore
let (observer, database) = app.add::<(
    bindings::QueryObserverPlugin,
    bindings::DatabaseConnectorPlugin,
)>("database", bytes)?;

runtime.component(observer).on_query(query)?;
runtime.component(database).execute(statement)?;
```

Both handles refer to the same component store and instance. Binding worlds export exactly one interface, while tuple addition validates all requested capabilities atomically; unrelated roles require no common plugin trait or world hierarchy.

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

Applications can define several plugin roles. A sibling-only provider can use a narrow export-only admission world, and unrelated plugin kinds do not need to share one artificial universal interface.

## Host integration

Lockgate constructs one `HostContext<S>` per component. The context always contains the application-assigned component name and resource table, while every context shares the application state `S`. A stateless application uses `S = ()`, so host state is always initialized.

```rust,ignore
impl bindings::myapp::host::services::Host for HostContext<()> {
    fn log(&mut self, message: String) {
        println!("{}: {message}", self.component_name());
    }
}
```

State is shared across component contexts. Mutable state uses an explicit synchronization primitive:

```rust,ignore
struct AppState {
    logger: Logger,
    counters: Mutex<HashMap<String, usize>>,
}

impl bindings::myapp::host::services::Host for HostContext<AppState> {
    fn log(&mut self, message: String) {
        self.state().logger.log(self.component_name(), message);
    }
}

let mut app = Application::new(AppState::new())?;
```

Host implementations use `HostContext::state`, `resources_mut`, and `component_name`. Component-specific data can be keyed by `component_name` inside the shared state.

A component may implement more than one application role without creating another instance:

```rust,ignore
let (runnable, reader) = app.add::<(
    bindings::RunnablePlugin,
    bindings::FileReaderPlugin,
)>("plugin", bytes)?;
```

Multi-role addition validates every requested role atomically and merges their host-interface requirements. Canonical installers are deduplicated, and every returned typed handle accesses the same component store.

## Enforcement lifecycle

The private catalog owned by `Application` compiles the exact supplied bytes, then uses `wit_component::decode` and Wasmtime component types to validate their real imports, exports, and function signatures. Artifact insertion is always admitted against a generated binding role.

Every artifact added to an application is instantiated when the application runs. Capability grants control authority rather than membership, and use application-owned handles. Host imports and sibling links remain distinct grants:

- `.allow_host_import(component, interface)` permits the embedding application to implement that exact interface when it is both imported by the artifact and declared by an admitted binding role.
- `.link(caller, provider)` permits the provider to satisfy matching sibling imports on the caller.
- Directory grants add narrowly scoped WASI preopens.

Before creating stores, runtime construction:

1. requires every custom import to have either a host grant or exactly one linked sibling provider;
2. resolves every function required from that sibling provider;
3. rejects async functions and values that cannot cross independent stores;
4. compares complete Wasmtime structural signatures;
5. installs only admitted host interfaces explicitly granted to that component;
6. prepares every complete linker with Wasmtime before application state or component stores are created.

Host-facing interfaces are not subjected to sibling cross-store restrictions. Their generated bindings and Wasmtime perform the relevant type checking.

Each component receives its own Wasmtime `Store`, resource table, WASI context, and fuel budget, while referencing the same initialized application state. Stores begin with no preopens and networking denied. Sibling forwarding uses one mutex per component and an identity-based call stack that rejects cycles and depths greater than eight before locking a callee. A trapping call marks only that component unhealthy.

## Repository layout

```text
src/
  application.rs         typed assembly, application state, and host installers
  binding.rs             generated binding admission and loading contract
  catalog.rs             artifact identity and decoded component metadata
  grants.rs              retained host, sibling, and WASI grants
  plan.rs                private provider resolution and preflight validation
  plugin.rs              structural type inspection and cross-store validation
  runtime.rs             isolated stores, host context, and sibling forwarding
lockgate-macros/          shared application-world binding generation
lockgate-schema/          shared canonical WIT contract metadata
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
scripts/public-api
```

The API inventory command generates rustdoc JSON with `cargo rustdoc`, then reports root exports, inherent methods, enum variants, and the separate hidden ABI used by generated code. The root library has no build script and does not require `cargo-component` to compile or test. The demo owns guest compilation and stages generated Wasm under Cargo's `OUT_DIR`.

## Add a demo component

1. Choose or add a narrow application admission world for the component; include host-visible exports and imports only when needed.
2. Define the component world under its own `wit/world.wit`, importing host services and sibling packages explicitly.
3. Use ordinary generated guest bindings for host and sibling calls.
4. List related admission worlds in a `lockgate::bindings!` invocation and implement each shared host interface once for `HostContext<S>`.
5. Add every artifact with `app.add::<GeneratedBinding>`.
6. Grant each host import, sibling link, and WASI capability separately; `Application` configures authorized host bindings automatically.
7. Add the component ID to `IDS` in `examples/demo/build.rs`.

## Current limits

The flake pins Rust 1.97.1, `cargo-component` 0.21.1, `wasm-tools` 1.254.0, and `wkg` 0.15.1. Rust dependencies pin Wasmtime 47.0.3 and `wit-component`/`wit-parser` 0.255.0.

- Sibling forwarding supports synchronous functions whose values can move between independent stores. Resource handles, `error-context`, futures, and streams cannot cross that boundary.
- Wasmtime 47's public component type conversion does not expose fixed-length lists and contains an unimplemented conversion for them. Lockgate rejects those artifacts before runtime type introspection.
- Sibling provider cycles are linkable because forwarding closures resolve stores only when called; runtime guards reject re-entry. A component that calls an import during its own instantiation can still fail when its provider store is not available yet.
- The runtime is synchronous. Application-generated host bindings may use richer ordinary WIT types than sibling forwarding, but async execution is not currently configured.
- Networking remains fail-closed because Wasmtime's current socket policy callback exposes resolved addresses rather than the requested hostname.
