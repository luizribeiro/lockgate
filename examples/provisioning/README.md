# Provisioning: policy v2 end to end

This example is the reviewer-facing tour of Lockgate's application policy API.
It models a VM service in memory so every interesting line is about authority,
not virtualization.

The authority vocabulary lives in the small, shared `provisioning-policy`
crate. `permissions::vm` declares two scope types and four typed permissions:
`CREATE` is governed by `PoolScope`; `EXEC` and `DESTROY` by `InstanceScope`;
and `LIST_POOLS` is unscoped. The host and guest compile those same Rust
definitions, so neither side reproduces stable atom strings.

The guest requests its maximum authority with typed constants:

```rust
const REQUIRED: &[Need] = &[
    vm::CREATE.need(&[ScopeRef::literal("gpu")]),
    vm::EXEC.need(&[
        ScopeRef::literal("pool:gpu"),
        ScopeRef::literal("created-by-caller"),
    ]),
    vm::DESTROY.need(&[ScopeRef::literal("created-by-caller")]),
];
```

The host registers that vocabulary, prepares the component, calls
`prepared.accept_all()`, and admits immutable effective grants. Its real WIT
implementation is annotated with `#[lockgate::guarded]`: each VM operation has
one `#[requires(...)]`, while `protocol-version` is explicitly
`no_capability_required`. There are no hand-written grant lookups in the host.

`MockVm` reports every membership relevant to the caller. Every VM belongs to
its pool; one created by the calling plugin additionally belongs to
`CreatedByCaller`. The async resolver maps a WIT string ID to that application
resource. Lockgate allows a call when an effective granted scope contains any
reported membership and denies before the method body otherwise.

After preparation and all-or-nothing acceptance, the plugin holds exactly the
grants it explicitly requested:

| Atom | Effective scopes |
| --- | --- |
| `vm.create` | `gpu` |
| `vm.exec` | `pool:gpu`, `created-by-caller` |
| `vm.destroy` | `created-by-caller` |

`vm.list-pools` is not requested and therefore is not effective. Policy v2
does not narrow broad requests and does not derive `exec` or `destroy` from
`create`; grant limits and implication graphs are deliberately deferred.

Build the guest and run the host from the repository root:

```bash
nix develop -c cargo build --manifest-path examples/provisioning/plugin/Cargo.toml --target wasm32-wasip2 --release
nix develop -c cargo run -p provisioning-host -- examples/provisioning/plugin/target/wasm32-wasip2/release/provisioning_plugin.wasm
```

The run creates a GPU VM, executes its own VM and an existing GPU VM, rejects
an unrelated CPU VM, then destroys the created VM:

```text
protocol version: 1
allowed: vm.create pool=gpu -> gpu/vm-1
allowed: vm.exec vm=gpu/vm-1 via=created-by-caller
allowed: vm.exec vm=gpu/base via=pool:gpu
denied: vm.exec vm=cpu/base atom=vm.exec
allowed: vm.destroy vm=gpu/vm-1 via=created-by-caller
plugin outcome: explicit create/exec/destroy grants enforced by membership
```
