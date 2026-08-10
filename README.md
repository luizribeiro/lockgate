# Lockgate

Lockgate is a small, auditable Rust library for capability-scoped WebAssembly component applications. The embedding application owns its host/plugin WIT contracts and uses generated Wasmtime bindings. Lockgate discovers sibling interfaces from component binaries, resolves authorized providers, and forwards sibling calls between isolated stores without compiling those application-specific APIs into the library.

The design keeps API knowledge and implementation selection separate:

- Host-to-guest exports and guest-to-host imports use application-owned WIT and generated bindings.
- Sibling callers and providers compile against their shared WIT, but Lockgate learns that contract from their artifacts.
- Application grants choose which concrete component satisfies each sibling import and which host interfaces each component may import.
- Lockgate uses runtime component values only inside the sibling forwarding implementation. There is no guest-visible dynamic registry or public raw host-call API.

## The model

Lockgate keeps three kinds of identity separate:

- Each artifact embeds a stable plugin ID, display name, and implementation version.
- Package, world, interface, and function names come from the WIT embedded in the component.
- Each admitted artifact receives an opaque `ComponentId`, so several instances may carry the same plugin metadata.

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
}

impl bindings::myapp::host::services::HostWithStore<
    lockgate::__private::PluginStore<()>,
> for HostContext<()> {
    async fn log(
        accessor: &wasmtime::component::Accessor<
            lockgate::__private::PluginStore<()>,
            Self,
        >,
        message: String,
    ) {
        accessor.with(|mut access| {
            println!("{}: {message}", access.get().plugin().id());
        });
    }
}

let mut app = Application::new(())?;
let greeter = app.add::<bindings::GreeterPlugin>(greeter_wasm)?;
let caller = app.add::<bindings::RunnablePlugin>(caller_wasm)?;

let runtime = app
    .link(caller, greeter)?
    .allow_host_import(caller, "myapp:host/services@1.0.0")?
    .run()
    .await?;

let result = runtime.component(caller).run().await?;
```

`lockgate::bindings!` parses all listed application worlds together. It generates every imported host interface once, remaps each Wasmtime world to those shared bindings, and emits export contracts plus canonical host-interface installers. The application therefore implements `myapp:host/services@1.0.0` once even when several plugin roles import it.

`Application::add::<Role>` requires and validates embedded Lockgate plugin metadata, then compares each requested generated export contract with WIT decoded from the artifact. A single role returns one typed component handle; a tuple of roles returns a matching tuple of handles after every role validates atomically. It also retains host-interface installers specialized for the application's state type. Even sibling-only providers use a narrow export-only role. Runtime construction installs only explicitly granted host interfaces and prepares every complete linker before creating stores.

`Runtime::component` turns a typed handle into a lightweight generated client. Each binding role represents one exported interface, so its WIT functions become async Rust methods directly on that client while Lockgate keeps Wasmtime stores, binding construction, fuel, locking, and trap health internal. A WIT `result<T, E>` remains inside the outer `Result<_, RuntimeError>`, so domain errors do not mark a component unhealthy.

The generated binding implementation exists only when `HostContext<S>` implements every host trait imported by that role. A missing host implementation is therefore a compile-time error at `Application::add`, while artifact admission and grant validation remain runtime checks over the supplied bytes and grants.

The runnable example contains the complete integration in `examples/demo/src/main.rs`.

`examples/coding-agent` builds a small Ratatui coding agent with replaceable WebAssembly provider
and tool plugins. Its typed provider protocol, Inkling adapter, and read-only workspace tools are
documented in `examples/coding-agent/README.md`.

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
)>(bytes)?;

runtime.component(observer).on_query(query).await?;
runtime.component(database).execute(statement).await?;
```

Both handles refer to the same component store and instance. Binding worlds export exactly one interface, while tuple addition validates all requested capabilities atomically; unrelated roles require no common plugin trait or world hierarchy.

If an application eventually needs guest-selected routing, it should define a domain-specific typed WIT service such as `render-with(provider, document)` rather than a universal string-and-value invocation protocol.

## WIT ownership

Lockgate owns no application WIT package. The demo application owns `demo:host`, which defines its host services and host-visible plugin roles:

```wit
package demo:host@0.1.0;

interface services {
  log: async func(message: string);
}

interface runnable {
  run: async func() -> result<string, string>;
}

world runnable-plugin {
  import services;
  export runnable;
}
```

The caller and filereader consume those contracts. The separately versioned `demo:greeter` package is shared only by the sibling caller and provider. Lockgate itself compiles against neither package.

Applications can define several plugin roles. A sibling-only provider can use a narrow export-only admission world, and unrelated plugin kinds do not need to share one artificial universal interface.

## Plugin authoring

Rust plugin authors depend on the lightweight `lockgate-plugin` crate rather than the host runtime. Its binding macro generates ordinary `wit-bindgen` guest bindings and embeds a versioned, language-neutral `lockgate:plugin` custom section:

```rust,ignore
mod bindings {
    lockgate_plugin::bindings!({
        path: "wit",
        world: "greeter",
        metadata: {
            id: "com.example.greeter",
            name: "Example Greeter",
            version: "1.2.0",
            description: "Returns localized greetings",
        },
    });
}
```

`id`, `name`, and a SemVer `version` are required. `description`, `license`, `repository`, and `homepage` are optional. The format is independent of Rust so other guest toolchains can emit the same custom section. Metadata is self-declared and never grants authority; Lockgate derives capabilities from actual imports plus application grants. Artifact provenance requires an external signature, trusted registry, or content digest.

Non-Rust tooling writes UTF-8 JSON directly into a custom section named `lockgate:plugin`; an admissible artifact contains exactly one such section. Format 1 has this shape:

```json
{
  "format": 1,
  "id": "com.example.greeter",
  "name": "Example Greeter",
  "version": "1.2.0",
  "description": "Returns localized greetings"
}
```

## Host integration

Lockgate constructs one `HostContext<S>` per component. The context always contains the plugin's embedded metadata and resource table, while every context shares the application state `S`. A stateless application uses `S = ()`, so host state is always initialized.

```rust,ignore
impl bindings::myapp::host::services::Host for HostContext<()> {
}
```

State is shared across component contexts. Mutable state uses an explicit synchronization primitive:

```rust,ignore
struct AppState {
    logger: Logger,
    counters: Mutex<HashMap<String, usize>>,
}

impl bindings::myapp::host::services::Host for HostContext<AppState> {}

impl bindings::myapp::host::services::HostWithStore<
    lockgate::__private::PluginStore<AppState>,
> for HostContext<AppState> {
    async fn log(
        accessor: &wasmtime::component::Accessor<
            lockgate::__private::PluginStore<AppState>,
            Self,
        >,
        message: String,
    ) {
        accessor.with(|mut access| {
            let context = access.get();
            context.state().logger.log(context.plugin().id(), message);
        });
    }
}

let mut app = Application::new(AppState::new())?;
```

Host implementations use `HostContext::state`, `resources_mut`, and `plugin`. `Application::metadata` makes the same metadata available immediately after admission without instantiating the plugin.

A component may implement more than one application role without creating another instance:

```rust,ignore
let (runnable, reader) = app.add::<(
    bindings::RunnablePlugin,
    bindings::FileReaderPlugin,
)>(bytes)?;
```

Multi-role addition validates every requested role atomically and merges their host-interface requirements. Canonical installers are deduplicated, and every returned typed handle accesses the same component store.

## Enforcement lifecycle

The private catalog owned by `Application` compiles the exact supplied bytes, then uses `wit_component::decode` and Wasmtime component types to validate their real imports, exports, and function signatures. Artifact insertion is always admitted against a generated binding role.

Every artifact added to an application is instantiated when the application runs. Capability grants control authority rather than membership, and use application-owned handles. Host imports and sibling links remain distinct grants:

- `.allow_host_import(component, interface)` permits the embedding application to implement that exact interface when it is both imported by the artifact and declared by an admitted binding role.
- `.link(caller, provider)` permits the provider to satisfy matching sibling imports on the caller.
- `.allow_outbound_http(component)` permits a component that imports `wasi:http/client@0.3.0` to make outbound HTTP requests.
- Directory grants add narrowly scoped WASI preopens.

Before creating stores, runtime construction:

1. requires every custom import to have either a host grant or exactly one linked sibling provider;
2. resolves every function required from that sibling provider;
3. rejects values that cannot cross independent stores;
4. compares complete Wasmtime structural signatures;
5. installs only admitted host interfaces explicitly granted to that component;
6. prepares every complete linker with Wasmtime before application state or component stores are created.

Host-facing interfaces are not subjected to sibling cross-store restrictions. Their generated bindings and Wasmtime perform the relevant type checking.

Each component receives its own Wasmtime `Store`, resource table, WASI context, and fuel budget, while referencing the same initialized application state. Stores begin with no preopens and networking denied unless the application grants outbound WASI HTTP. Sibling forwarding uses one mutex per component and an identity-based call stack that rejects cycles and depths greater than eight before locking a callee. A trapping call marks only that component unhealthy.

The default instruction budget is 100,000 units of Wasmtime fuel per component call and during
initialization. Applications with more substantial plugins can set a different bounded value with
`Application::fuel_per_call`; Lockgate restores it at every call boundary.

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
lockgate-plugin/          lightweight Rust guest API
lockgate-plugin-macros/   guest binding and plugin metadata generation
lockgate-schema/          shared WIT contracts and plugin metadata format
examples/demo/
  build.rs               builds and stages executable demo components
  src/main.rs            generated host bindings and the narrated lifecycle
  components/            three independent Rust component crates
  wit/worlds.wit         guest implementation worlds
  wit/deps/              shared host and sibling WIT contracts
  sandbox/               demo filesystem input
examples/coding-agent/   TUI agent with provider and tool component roles
```

The guest components form a small workspace with one dependency set and lockfile, independent from the host workspace. Each guest depends only on `lockgate-plugin`; Cargo compiles the generated bindings and metadata directly to native `wasm32-wasip2` components.

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

The API inventory command generates rustdoc JSON with `cargo rustdoc`, then reports root exports, inherent methods, enum variants, and the separate hidden ABI used by generated code. The root library has no build script. The demo builds its guests with Cargo's nightly `build-std` support and stages the native `wasm32-wasip2` components under Cargo's `OUT_DIR`.

## Add a demo component

1. Choose or add a narrow application admission world for the component; include host-visible exports and imports only when needed.
2. Add the component's implementation world to `examples/demo/wit/worlds.wit`, importing host services and sibling packages explicitly.
3. Generate guest bindings and embed required metadata with `lockgate_plugin::bindings!`, then build for `wasm32-wasip2`.
4. List related admission worlds in a `lockgate::bindings!` invocation and implement each shared host interface once for `HostContext<S>`.
5. Add every artifact with `app.add::<GeneratedBinding>`.
6. Grant each host import, sibling link, and WASI capability separately; `Application` configures authorized host bindings automatically.
7. Add the component ID to `IDS` in `examples/demo/build.rs`.

## Current limits

The flake supplies nightly Rust with `rust-src`, builds the WASIp2 standard library on demand, and provides wasi-sdk 34 RC2's sysroot. Lockgate hosts both WASIp2 and WASIp3 interfaces so plugins can use the P2 standard library alongside newer component APIs.

- Sibling forwarding supports native async functions whose parameters and results can move between independent stores. Resource handles, `error-context`, futures, and streams cannot cross that boundary.
- Sibling provider cycles are linkable because forwarding closures resolve stores only when called; runtime guards reject re-entry. A component that calls an import during its own instantiation can still fail when its provider store is not available yet.
- Networking remains fail-closed because Wasmtime's current socket policy callback exposes resolved addresses rather than the requested hostname.
- Plugin metadata lives in a WebAssembly custom section. Generic stripping tools may remove custom sections, making the resulting artifact inadmissible until metadata is restored.
