mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use lockgate::__private::{StoreCtx, wasmtime};
use lockgate::{
    CallError, HostBuilder, HostImports, InvocationCtx, PluginConfig, Role, RoleInvocation,
    RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

struct Imports {
    clones: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl Clone for Imports {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
            calls: Arc::clone(&self.calls),
        }
    }
}

impl HostImports<u32> for Imports {
    fn policy_metadata()
    -> Result<lockgate::__private::HostImportPolicyMetadata, lockgate::HostImportPolicyError> {
        // This test deliberately exercises the unsupported handwritten linker
        // seam, so its policy-wiring bypass must be explicit and reviewable.
        Ok(lockgate::__private::HostImportPolicyMetadata::__empty())
    }

    fn add_to_linker(
        &self,
        linker: &mut wasmtime::component::Linker<StoreCtx<u32>>,
        _interfaces: &[String],
    ) -> wasmtime::Result<()> {
        linker
            .instance("test:manual/host")?
            .func_wrap_async("echo", |mut host, (): ()| {
                Box::new(async move {
                    let (imports, data, _plugin, _jobs, _resources) =
                        host.data_mut()
                            .host_parts::<Imports, lockgate::PluginHandle>();
                    imports.calls.fetch_add(1, Ordering::SeqCst);
                    Ok((*data,))
                })
            })?;
        Ok(())
    }
}

struct GuestRole;

struct GuestClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for GuestRole {
    const INTERFACE: &'static str = "test:manual/guest";

    type Client<'a, S>
        = GuestClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        GuestClient(invocation)
    }
}

impl GuestClient<'_, u32> {
    async fn call(&self, data: u32) -> Result<u32, CallError> {
        let values = self
            .0
            .invoke("call", &[], InvocationCtx::new(data, budget()))
            .await?;
        match values.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape("expected one u32 result")),
        }
    }
}

fn budget() -> lockgate::BudgetClass {
    lockgate::BudgetClass::Bounded {
        fuel: 1_000_000,
        deadline: common::INVOCATION_DEADLINE,
    }
}

fn component() -> Vec<u8> {
    let component = wat::parse_str(
        r#"(component
            (type $host (instance
                (export "echo" (func (result u32)))
            ))
            (import "test:manual/host" (instance $host-instance (type $host)))
            (alias export $host-instance "echo" (func $echo))
            (core func $echo-lowered (canon lower (func $echo)))
            (core module $module
                (import "" "echo" (func $echo (result i32)))
                (func (export "call") (result i32)
                    call $echo)
            )
            (core instance $instance
                (instantiate $module
                    (with "" (instance
                        (export "echo" (func $echo-lowered))
                    ))
                )
            )
            (func $call (result u32) (canon lift (core func $instance "call")))
            (instance $guest
                (export "call" (func $call))
            )
            (export "test:manual/guest" (instance $guest))
        )"#,
    )
    .unwrap();
    let metadata = PluginMetadata::new("manual-import", "Manual import", "1.0").unwrap();
    common::sectioned_fixture(&component, &metadata)
}

#[tokio::test]
async fn handwritten_imports_receive_data_and_clone_per_call() {
    let clones = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut builder = HostBuilder::new(Imports {
        clones: Arc::clone(&clones),
        calls: Arc::clone(&calls),
    })
    .unwrap();
    let prepared = builder
        .prepare("manual-import", &component(), PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::new(11, budget()),
        )
        .await
        .unwrap();

    clones.store(0, Ordering::SeqCst);
    let host = builder.finish();
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    assert_eq!(guest.call(42).await.unwrap(), 42);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(clones.load(Ordering::SeqCst), 2);
}
