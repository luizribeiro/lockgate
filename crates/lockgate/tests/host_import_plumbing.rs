mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use lockgate::__private::{StoreCtx, wasmtime};
use lockgate::{
    CallError, Host, HostBuilder, HostImports, InvocationCtx, PluginConfig, PluginHandle, Role,
    RoleInvocation, RuntimeLimits, Value,
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

    async fn call_many(&self, data: u32, calls: u32) -> Result<u32, CallError> {
        let values = self
            .0
            .invoke(
                "call-many",
                &[Value::U32(calls)],
                InvocationCtx::new(data, budget()),
            )
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
                (func (export "call-many") (param $count i32) (result i32)
                    (local $index i32)
                    (local $value i32)
                    (block $done
                        (loop $next
                            local.get $index
                            local.get $count
                            i32.ge_u
                            br_if $done
                            call $echo
                            local.set $value
                            local.get $index
                            i32.const 1
                            i32.add
                            local.set $index
                            br $next
                        )
                    )
                    local.get $value
                )
            )
            (core instance $instance
                (instantiate $module
                    (with "" (instance
                        (export "echo" (func $echo-lowered))
                    ))
                )
            )
            (func $call (result u32) (canon lift (core func $instance "call")))
            (func $call-many (param "calls" u32) (result u32)
                (canon lift (core func $instance "call-many")))
            (instance $guest
                (export "call" (func $call))
                (export "call-many" (func $call-many))
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

async fn host_with_call_limit(
    max_host_import_calls: u64,
) -> (Host<u32>, PluginHandle, Arc<AtomicUsize>) {
    let clones = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut builder = HostBuilder::new(Imports {
        clones,
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
            RuntimeLimits {
                max_host_import_calls,
                ..RuntimeLimits::default()
            },
            InvocationCtx::new(0, budget()),
        )
        .await
        .unwrap();
    calls.store(0, Ordering::SeqCst);
    (builder.finish(), plugin, calls)
}

#[tokio::test]
async fn host_import_call_limit_stops_before_the_over_limit_call() {
    const LIMIT: u64 = 3;
    let (host, plugin, calls) = host_with_call_limit(LIMIT).await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    let error = guest.call_many(42, LIMIT as u32 + 1).await.unwrap_err();

    assert!(matches!(
        error,
        CallError::HostImportCallLimitExceeded { limit } if limit == LIMIT
    ));
    assert_eq!(calls.load(Ordering::SeqCst), LIMIT as usize);
}

#[tokio::test]
async fn host_import_call_counter_resets_for_each_under_limit_invocation() {
    const LIMIT: u64 = 3;
    let (host, plugin, calls) = host_with_call_limit(LIMIT).await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    assert_eq!(guest.call_many(41, LIMIT as u32 - 1).await.unwrap(), 41);
    assert_eq!(guest.call_many(42, LIMIT as u32 - 1).await.unwrap(), 42);
    assert_eq!(calls.load(Ordering::SeqCst), (LIMIT as usize - 1) * 2);
}

#[tokio::test]
async fn zero_host_import_call_limit_allows_many_calls() {
    const CALLS: u32 = 2_000;
    let (host, plugin, calls) = host_with_call_limit(0).await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    assert_eq!(guest.call_many(42, CALLS).await.unwrap(), 42);
    assert_eq!(calls.load(Ordering::SeqCst), CALLS as usize);
}
