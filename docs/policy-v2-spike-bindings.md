# Pinned host-binding mechanics decisions

This spike used Wasmtime `45af25f` through Lockgate's real audit host binding.
Source remains in `scratchpad/spike`; `cargo run` succeeds, while features
`self-borrow-fail` and `native-send-fail` preserve the negative diagnostics.

## 1. Generated-metadata exchange

### Finding

A codegen-owned companion plus one associated-const slot on each generated
`Host` trait provides a closed exchange. The impl attribute selects identities
that codegen created; generated `HostImports` code can validate the resulting
slice during `HostBuilder::new`. No inventory, registry, or user spelling of a
WIT name is involved.

The companion is preferable to overridable identity consts on `Host`: users may
fill the policy slot, but cannot redefine the binding's expected identities.

### Evidence

The compiling probe hand-writes the intended expansions:

```rust
struct AuditBinding;
impl AuditBinding {
    const INTERFACE: InterfaceIdentity = InterfaceIdentity {
        name: "example:audit/audit", version: None,
    };
    const LOG: MethodIdentity = MethodIdentity { rust_name: "log", wit_name: "log" };
    const METHODS: &'static [MethodIdentity] = &[Self::LOG];
}
trait Host { const POLICY_METHODS: &'static [PolicyMethod] = &[]; }
impl Host for Imports {
    const POLICY_METHODS: &'static [PolicyMethod] = &[PolicyMethod {
        interface: AuditBinding::INTERFACE, method: AuditBinding::LOG,
    }];
}
```

`HostBuilderLike::new::<Imports>()` reads `I::POLICY_METHODS` and checks both
fields against `AuditBinding::{INTERFACE, METHODS}`; the executable probe ran
successfully.

### Recommendation

For every imported interface, host binding codegen should emit a doc-hidden,
public companion beside `Host`: canonical package/interface name without
`@version`, a separate optional package version, an ordered
`{ rust_name, wit_name }` method slice, and one non-overridable const per Rust
method identifier.

Codegen should also add a default-empty `__LOCKGATE_POLICY_METHODS` associated
const to that interface's `Host` trait. The impl-block attribute consumes the
trait path and each annotated Rust method identifier, selects the matching
companion const, and fills that slot. Generated `HostImports` construction code
must compare the slot with the same companion before retaining the imports.
Keep the identity structs' fields private so macro output can copy generated
values but cannot manufacture identities.

## 2. Target-expression mechanics

### Finding

An outer shared borrow, followed by an explicit shared reborrow of the receiver,
works for owned parameters, `HostCtx` data fields, and even shared fields of
`self`. An expression that first takes an exclusive borrow of `self` conflicts
with the resolver's `&self` for the duration of the await.

### Evidence

These exact forms compile inside the generated `audit::Host::log` method:

```rust
let __target = &(message);
let _ = Resolve::resolve(&*self, cx.data().subject.as_str(), __target).await.expect("infallible");
let __target = &(cx.data().something);
let _ = Resolve::resolve(&*self, cx.data().subject.as_str(), __target).await.expect("infallible");
let __target = &(self.namespace); // shared self field also compiles
let _ = Resolve::resolve(&*self, cx.data().subject.as_str(), __target).await.expect("infallible");
```

The feature-gated `&(self.target_mut())` probe fails with E0502. Rust highlights
the mutable borrow at the expression, the immutable borrow at `&*self`, and the
later use at `__target`; the diagnostic is readable, but arrives from injected
code.

### Recommendation

Inject this shape (with the eventual error conversion in place of `?`):

```rust
let __lockgate_target = &(EXPR);
let __lockgate_resource = Resolve::resolve(&*self, __lockgate_subject,
    __lockgate_target).await.map_err(/* policy error conversion */)?;
```

Referenced method arguments must use identifier patterns; the expression must
type-check as a shared target and its borrowed value must be `Sync` because it
is held by a `Send` future across `.await`. The context parameter need not have
a fixed name: the attribute parses the user's expression, so `cx.data().field`
uses whatever identifier the impl declared.

For the first implementation, accept parameter paths and paths rooted at
`HostCtx::data()`, and reject every expression containing `self` with an
attribute diagnostic. A shared self field is mechanically possible, but a proc
macro cannot type-resolve whether `self.method()` borrows shared or exclusive;
the conservative rule avoids inconsistent E0502 failures and dependence on the
per-call cloned imports value. Also reject destructured referenced parameters.

## 3. Async resolver spelling

### Finding

Bare native `async fn` in the resolver trait does not promise a `Send` future to
generic generated code. Boxing promises it but allocates. The pinned toolchain
already uses the better middle ground for generated host traits: declare RPITIT
with `+ Send`, while application impls retain `async fn` syntax.

The generated Lockgate host method and adapter both return
`impl Future<...> + Send` (`crates/lockgate-macros/src/lib.rs:1402` and `:1439`).
`HostImports` already makes the concrete imports type `Send + Sync + 'static`.

### Evidence

This trait and an `async fn` impl compile when awaited inside the real audit
host method:

```rust
trait Resolve<T: ?Sized + Sync>: Send + Sync {
    type Resource: Send;
    type Error: Send;
    fn resolve<'a>(&'a self, subject: &'a str, target: &'a T)
        -> impl Future<Output = Result<Self::Resource, Self::Error>> + Send + 'a;
}
impl<T: ?Sized + Sync> Resolve<T> for Imports {
    async fn resolve<'a>(&'a self, subject: &'a str, target: &'a T)
        -> Result<Self::Resource, Self::Error> { /* ... */ }
}
```

The probe keeps all three borrows live across an await. Its bare native-trait
variant fails the caller's `Future + Send` assertion; rustc recommends
declaring `-> impl Future<...> + Send`.

### Recommendation

Use the signature above, substituting the final subject type. Do not box and do
not declare bare `async fn` in the public trait. Keep the single call lifetime;
no borrow needs to be `'static`. Require `Self` and borrowed target/subject
types to be `Sync`, the returned future to be `Send`, and `Resource: Send` so a
generated host future may retain it across later awaits. Retain `Error: Send`
for the same generated boundary and add only the final error-conversion bound.

## 4. Resource-method access

### Finding

Pinned bindgen passes a WIT resource method an opaque
`Resource<Representation>` plus `&mut self`; it does not dereference the handle.
The host reaches the live representation through a `ResourceTable` owned by or
borrowed into its state/view: `table.get(&handle)`, `get_mut`, `delete`, and
`push` for constructors.

Lockgate does not yet provide that path. `StoreCtx` contains invocation data,
cloned imports, plugin, jobs, and settings (`crates/lockgate/src/exec/mod.rs:278`),
while `HostCtx` exposes only data, plugin, and jobs (`crates/lockgate/src/lib.rs:87`).
The adapter clones imports out of the Store before awaiting the application
method (`crates/lockgate-macros/src/lib.rs:1444`).

### Evidence

The pinned expanded binding's `HostBar::method_a` takes `Resource<Bar>`
(`~/.cargo/git/checkouts/wasmtime-ae46461068d65b15/45af25f/crates/component-macro/tests/expanded/resources-import_async.rs:432`).
`ResourceTable` documents the handle-to-value mapping and implements `get`,
`get_mut`, and `delete` (`.../crates/wasmtime/src/runtime/component/resource_table.rs:35,304-341`).
The pinned WASI HTTP host uses exactly `self.table.get(&fields)` and
`get_mut(&fields)` (`.../crates/wasi-http/src/p2/types_impl.rs:35-105`); its
`WasiHttpCtxView` carries `&mut ResourceTable` (`.../crates/wasi-http/src/ctx.rs:91-109`).

### Recommendation

Add one per-Store resource table when resource imports are supported, and give
generated resolution code a narrow resource-resolution context that mediates
lookup. Do not place the table directly in `Imports`: imports are cloned per
host call, whereas resource handles belong to a component Store. The context
should expose only the live representation/capability facts needed for
resolution and keep table borrows scoped around async work; do not add a broad
table escape hatch to public `HostCtx`.

## Consequences for the upcoming implementation

- Extend host codegen first: companion metadata, the per-interface policy slot,
  and construction validation are prerequisites for the impl attribute.
- Give target parsing a small v1 grammar: named parameters and `HostCtx::data()`
  paths, with `self` and destructuring rejected explicitly.
- Declare resolver futures with RPITIT `+ Send`; allow application impls to use
  `async fn`, matching the existing generated-host convention.
- Design per-Store resource-table ownership and a narrow resolution context
  before claiming policy support for WIT resource methods.
