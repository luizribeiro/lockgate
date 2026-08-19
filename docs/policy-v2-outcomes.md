# Policy v2 prototype outcomes

This records the answers that the implementation actually landed on for the
prototype questions in policy-v2 §14. It describes the shipped code, including
one facade question that remains open, rather than restating the proposed API.

## 14.1 WIT resource method integration

Chunk J (`97051c5`, `b55c813`) settled on one `ResourceStore` per invocation
Store and a narrow `ResolveCtx`. The context exposes the caller, immutable call
context, and typed insert/get/delete operations; it exposes neither grants nor
the underlying `ResourceTable`. Generated resource adapters route a direct
`Resource<T>` parameter through `ResolveScopedResourceHandle` in
`crates/lockgate-macros/src/guarded.rs`, while
`crates/lockgate/src/policy/resources.rs` owns the table and live-reference
rules.

Lookup wins before application resolution and authorization. A stale or
wrong-typed handle becomes `ResourceLookupError` through the method's ordinary
error conversion. Generated destructors alone treat an already-absent entry as
benign so natural guest drop cannot turn the prior typed failure into a trap.

The default table retains an `Arc` to the live representation. The resolver,
`scopes_for`, and effective-grant check still run on every method call; no
membership facts are cached at acquisition. The fresh-identity, live-reference,
and versioned-snapshot shapes are compared in the resource policy unit tests,
and `crates/lockgate/tests/resource_guarded_bindings.rs` mutates membership
between calls and exercises stale handles through generated bindings.

## 14.2 Target-expression mechanics

Chunks I and K (`6b22319`, `5bc0da3`, `ea4d99f`) use a deliberately small
grammar implemented by `crates/lockgate-macros/src/guarded.rs`: field paths
rooted at a named method parameter, or field paths rooted at that method's
`HostCtx::data()`. The context parameter is found by its `HostCtx` type and need
not be named `cx`. Destructured roots, `self`, control flow, and paths through a
resource handle are rejected with attribute diagnostics.

The expansion binds `&(target_expression)` once, awaits the selected resolver,
then checks the result before entering the original body. Ordinary Rust type
checking supplies local errors for non-`Sync` borrows and incompatible target
types. Expansion snapshots and the guarded trybuild suite pin both the argument
and call-context forms.

## 14.3 Generated wiring metadata

Chunks F1 and F2 (`495c0b5` through `980a521`) landed on codegen-owned companion
metadata. Each generated interface has an unforgeable `__LockgateBinding`
identity and ordered method constants; `#[guarded]` fills the associated policy
slice with those constants. Generated `HostImports::policy_metadata()` validates
all slices during `HostBuilder::new`, then preparation applies the selective
wiring rule in `crates/lockgate/src/lifecycle.rs`.

The raw generated trait keeps an empty default as an internal codegen seam, but
`validate_interface_policy_parts` rejects an empty, partial, duplicate,
unknown, or wrong-interface slice. Thus omitting `#[guarded]` is a construction
error, not an unguarded linker path. No runtime inventory or user-authored WIT
identity string is involved.

## 14.4 Async resolver surface

Chunk H (`e461720`) chose RPITIT with an explicit `+ Send` future in
`ResolveScopedResource` and, later, `ResolveScopedResourceHandle`. Resource and
error associated types are `Send`, borrowed arguments are `Sync`, and one call
lifetime covers the resolver, subject, and target. Application implementations
can write ordinary `async fn` without boxing or allocation. The public spelling
lives in `crates/lockgate/src/policy/resources.rs`.

## 14.5 Resolver failure and information disclosure

Chunks I and J made the ordering explicit: target resolution happens before the
effective-grant decision. A lookup may therefore reveal existence unless the
application chooses an opaque domain error. Lockgate keeps resolver failures
and `PermissionDenied` typed internally, while the guarded method's ordinary
`From` conversions decide whether the WIT error distinguishes or coalesces
them. The framework does not force one disclosure policy on every application.

The generated order is visible in `crates/lockgate-macros/src/guarded.rs`; the
distinct and coalesced conversions are exercised by the guarded argument and
resource fixtures under `crates/lockgate/tests/`.

## 14.6 Check-then-act validity

The first slice remains check-then-act. An argument resolver returns an owned
resource or authorization snapshot; a resource-handle resolver may retain an
`Arc` to a live representation. The guard authorizes that value and then enters
the original method body. Lockgate does not yet return an `Authorized<T>`, hold
a transaction, or otherwise make classification and mutation atomic.

`ScopedResource` and `ResolveCtx` rustdoc in
`crates/lockgate/src/policy/resources.rs` assigns validity and invalidation of
application-owned snapshots to the application. A retained authorized resource
remains a later slice, to be driven by the first capability that needs atomic
check-and-act semantics.

## 14.7 SDK facade assembly

The generic half landed: `lockgate-plugin` re-exports `Need`, `Needs`,
`ScopeRef`, and the typed permission handles, while an application-owned policy
crate can share its contract with host and guest. The provisioning showcase
uses `provisioning-policy::permissions::vm` on both sides.

The single-dependency application SDK facade did **not** land. Both `generate!`
and `export!` resolve a direct Cargo dependency named `lockgate-plugin` in
`crates/lockgate-plugin-macros/src/lib.rs`; merely re-exporting those macros
through an application SDK is insufficient. Consequently the example guest
depends directly on both `lockgate-plugin` and `provisioning-policy`, and uses
the minimum `permissions::vm` layout rather than a flattened `vm` facade. The
dependency direction is proven, but the one-dependency packaging mechanism
remains an explicit follow-up.

## Step-10 built-in compatibility inspection

This checkout contains no production filesystem-preopen or HTTP-origin
enforcement to regression-test. There is no `fs` or `http` capability contract,
no preopen/origin adapter, and no Wasmtime WASI or WASI-HTTP dependency under
`crates/lockgate`. The older `grants-v1` line contains the admission join and
provisioning example but never implemented the filesystem and HTTP work that
the original execution plan scheduled as steps 24 and 25.
Therefore this chunk cannot truthfully claim that those nonexistent consumers
were verified against `EffectiveGrants`, and it adds no proxy test labeled as
real built-in enforcement.

Runtime limits are present and remain independent of policy. `RuntimeLimits`
still converts directly to `ExecLimits` in `crates/lockgate/src/lifecycle.rs`;
admission passes those limits to smoke instantiation and retains them on the
admitted artifact; `crates/lockgate/src/role.rs` passes them unchanged to every
fresh Store; and `crates/lockgate/src/exec/mod.rs` applies fuel and memory
limits. Existing lifecycle and detached-job tests continue to cover zero fuel,
zero memory, invocation fuel, and job quota behavior through the same
prepare/accept/admit flow.
