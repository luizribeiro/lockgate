use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lockgate::{
    HostBuilder, HostCtx, PermissionDenied, PluginConfig, PluginId, PluginSubject,
    ResolveScopedResource, RuntimeLimits, ScopedResource,
};
use provisioning_policy::permissions::vm::{self as vm_permissions, InstanceScope, PoolScope};

const PLUGIN_ID: &str = "provisioning";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/provisioning/plugin/target/wasm32-wasip2/release/provisioning_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/provisioning/plugin/Cargo.toml --target wasm32-wasip2 --release";

#[derive(Clone, Debug, PartialEq, Eq)]
struct MockVm {
    pool: String,
    created_by: Option<PluginId>,
}

impl MockVm {
    fn existing(pool: &str) -> Self {
        Self {
            pool: pool.to_owned(),
            created_by: None,
        }
    }

    fn memberships_for(&self, plugin_id: &PluginId) -> Vec<InstanceScope> {
        let mut memberships = vec![InstanceScope::Pool(self.pool.clone())];
        if self.created_by.as_ref() == Some(plugin_id) {
            memberships.push(InstanceScope::CreatedByCaller);
        }
        memberships
    }
}

impl ScopedResource<InstanceScope> for MockVm {
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<InstanceScope> {
        self.memberships_for(subject.plugin_id())
    }
}

#[derive(Clone)]
struct MockPool(String);

impl ScopedResource<PoolScope> for MockPool {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<PoolScope> {
        vec![PoolScope::Named(self.0.clone())]
    }
}

struct VmState {
    pools: BTreeSet<String>,
    vms: Mutex<BTreeMap<String, MockVm>>,
    next_id: Mutex<u64>,
}

#[derive(Clone)]
struct Imports {
    state: Arc<VmState>,
}

impl Imports {
    fn new() -> Self {
        let vms = [
            ("gpu/base".to_owned(), MockVm::existing("gpu")),
            ("cpu/base".to_owned(), MockVm::existing("cpu")),
        ]
        .into_iter()
        .collect();
        Self {
            state: Arc::new(VmState {
                pools: ["cpu".to_owned(), "gpu".to_owned()].into_iter().collect(),
                vms: Mutex::new(vms),
                next_id: Mutex::new(1),
            }),
        }
    }
}

#[derive(Debug)]
struct LookupError(String);

impl ResolveScopedResource<PoolScope, String> for Imports {
    type Resource = MockPool;
    type Error = LookupError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        pool: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        self.state
            .pools
            .contains(pool)
            .then(|| MockPool(pool.clone()))
            .ok_or_else(|| LookupError(format!("pool {pool}")))
    }
}

impl ResolveScopedResource<InstanceScope, String> for Imports {
    type Resource = MockVm;
    type Error = LookupError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        vm: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        self.state
            .vms
            .lock()
            .unwrap()
            .get(vm)
            .cloned()
            .ok_or_else(|| LookupError(format!("VM {vm}")))
    }
}

lockgate::host_bindings!({
    path: "../wit",
    world: "plugin",
    imports_type: Imports,
    data: (),
});

use provisioner::HostExt;

impl From<LookupError> for vm::VmError {
    fn from(error: LookupError) -> Self {
        Self::NotFound(error.0)
    }
}

impl From<PermissionDenied> for vm::VmError {
    fn from(error: PermissionDenied) -> Self {
        Self::Denied(format!("{}.{}", error.capability(), error.permission()))
    }
}

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = vm_permissions::CREATE, target = pool)]
    async fn create(&mut self, cx: HostCtx<'_, ()>, pool: String) -> Result<String, vm::VmError> {
        let mut next_id = self.state.next_id.lock().unwrap();
        let id = format!("{pool}/vm-{}", *next_id);
        *next_id += 1;
        self.state.vms.lock().unwrap().insert(
            id.clone(),
            MockVm {
                pool,
                created_by: Some(cx.subject().plugin_id().clone()),
            },
        );
        Ok(id)
    }

    #[lockgate::requires(permission = vm_permissions::EXEC, target = vm)]
    async fn exec(
        &mut self,
        _cx: HostCtx<'_, ()>,
        vm: String,
        command: String,
    ) -> Result<String, vm::VmError> {
        Ok(format!("executed `{command}` on {vm}"))
    }

    #[lockgate::requires(permission = vm_permissions::DESTROY, target = vm)]
    async fn destroy(&mut self, _cx: HostCtx<'_, ()>, vm: String) -> Result<(), vm::VmError> {
        self.state.vms.lock().unwrap().remove(&vm);
        Ok(())
    }

    #[lockgate::requires(permission = vm_permissions::LIST_POOLS)]
    async fn list_pools(&mut self, _cx: HostCtx<'_, ()>) -> Result<Vec<String>, vm::VmError> {
        Ok(self.state.pools.iter().cloned().collect())
    }

    #[lockgate::no_capability_required(reason = "returns only a static protocol version")]
    async fn protocol_version(&mut self, _cx: HostCtx<'_, ()>) -> String {
        "1".to_owned()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PLUGIN_PATH));
    let bytes = read_component(&path)?;

    let mut builder = HostBuilder::new(Imports::new())?.register::<vm_permissions::Contract>()?;
    let prepared = builder
        .prepare(PluginId::from(PLUGIN_ID), &bytes, PluginConfig::default())
        .await?;
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await?;
    let host = builder.finish();

    let lines = host
        .provisioner(&plugin)?
        .run()
        .await?
        .map_err(std::io::Error::other)?;
    for line in lines {
        println!("{line}");
    }
    host.shutdown().await;
    Ok(())
}

fn read_component(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    std::fs::read(path).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "could not read plugin component at `{}`: {source}\n\
                 build the plugin first with:\n  {BUILD_COMMAND}",
                path.display()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{InstanceScope, MockVm};

    #[test]
    fn golden_vm_membership_table_preserves_pool_and_provenance() {
        let rows = [
            (
                MockVm {
                    pool: "gpu".into(),
                    created_by: None,
                },
                "plugin-a",
                vec![InstanceScope::Pool("gpu".into())],
            ),
            (
                MockVm {
                    pool: "gpu".into(),
                    created_by: Some(lockgate::PluginId::from("plugin-a")),
                },
                "plugin-a",
                vec![
                    InstanceScope::Pool("gpu".into()),
                    InstanceScope::CreatedByCaller,
                ],
            ),
            (
                MockVm {
                    pool: "gpu".into(),
                    created_by: Some(lockgate::PluginId::from("plugin-a")),
                },
                "plugin-b",
                vec![InstanceScope::Pool("gpu".into())],
            ),
        ];

        for (vm, caller, expected) in rows {
            assert_eq!(
                vm.memberships_for(&lockgate::PluginId::from(caller)),
                expected
            );
        }
    }
}
