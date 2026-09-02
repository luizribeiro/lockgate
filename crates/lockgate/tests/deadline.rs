mod common;

use std::future::pending;
use std::time::Duration;

use lockgate::__private::{StoreCtx, wasmtime};
use lockgate::PluginId;
use lockgate::{
    AdmissionError, CallBudget, CallError, Host, HostBuilder, HostImports, PluginConfig,
    PluginHandle, Role, RoleInvocation, RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

const CALL_FUEL: u64 = 1_000_000;
const COMPUTE_FUEL: u64 = 1_000_000_000;

#[derive(Clone, Copy)]
struct BlockingImports;

impl HostImports<()> for BlockingImports {
    fn policy_metadata()
    -> Result<lockgate::__private::HostImportPolicyMetadata, lockgate::HostImportPolicyError> {
        Ok(lockgate::__private::HostImportPolicyMetadata::__empty())
    }

    fn add_to_linker(
        &self,
        linker: &mut wasmtime::component::Linker<StoreCtx<()>>,
        _interfaces: &[String],
    ) -> wasmtime::Result<()> {
        linker
            .instance("test:exec/host")?
            .func_wrap_concurrent("wait", |_, (): ()| {
                Box::pin(pending::<wasmtime::Result<()>>())
            })?;
        linker
            .instance("test:deadline/host")?
            .func_wrap_async("wait", |_, (): ()| {
                Box::new(pending::<wasmtime::Result<()>>())
            })?;
        Ok(())
    }
}

struct GuestRole;

struct GuestClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for GuestRole {
    const INTERFACE: &'static str = "test:exec/guest";

    type Budgets = ();

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

impl<S: Send + Sync + 'static> GuestClient<'_, S> {
    async fn value(&self, data: S, budget: CallBudget) -> Result<u32, CallError> {
        self.call_u32("value", data, budget).await
    }

    async fn suspend(&self, data: S, budget: CallBudget) -> Result<u32, CallError> {
        self.call_u32("suspend", data, budget).await
    }

    async fn spin(&self, data: S, budget: CallBudget) -> Result<u32, CallError> {
        self.call_u32("spin", data, budget).await
    }

    async fn call_u32(
        &self,
        function: &str,
        data: S,
        budget: CallBudget,
    ) -> Result<u32, CallError> {
        let results = self
            .0
            .invoke_with_budget(function, &[], data, budget)
            .await?;
        match results.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape(format!(
                "{function} returned a non-u32 result"
            ))),
        }
    }
}

async fn admitted_fixture() -> (Host<()>, PluginHandle) {
    let mut builder = HostBuilder::new(BlockingImports).unwrap();
    let metadata = PluginMetadata::new("deadline", "Deadline fixture", "1.0").unwrap();
    let bytes = common::sectioned_fixture(&common::EXEC_FIXTURE, &metadata);
    let prepared = builder
        .prepare(
            PluginId::try_from("deadline").unwrap(),
            &bytes,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits {
                max_host_import_calls: 0,
                ..RuntimeLimits::default()
            },
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

fn blocking_start_component() -> Vec<u8> {
    wat::parse_str(
        r#"(component
            (type $host (instance
                (export "wait" (func))
            ))
            (import "test:deadline/host" (instance $host-instance (type $host)))
            (alias export $host-instance "wait" (func $wait))
            (core func $wait-lowered (canon lower (func $wait)))
            (core module $module
                (import "" "wait" (func $wait))
                (func $start
                    call $wait)
                (start $start)
            )
            (core instance $instance
                (instantiate $module
                    (with "" (instance
                        (export "wait" (func $wait-lowered))
                    ))
                )
            )
        )"#,
    )
    .unwrap()
}

#[tokio::test]
async fn blocking_start_returns_a_smoke_deadline_error() {
    let deadline = Duration::from_millis(200);
    let mut builder = HostBuilder::new(BlockingImports)
        .unwrap()
        .admission_budget(CallBudget {
            fuel: CALL_FUEL,
            deadline,
        })
        .unwrap();
    let metadata = PluginMetadata::new("deadline-start", "Deadline start fixture", "1.0").unwrap();
    let bytes = common::sectioned_fixture(&blocking_start_component(), &metadata);
    let prepared = builder
        .prepare(
            PluginId::try_from("deadline-start").unwrap(),
            &bytes,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let error = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AdmissionError::SmokeFailure { ref message }
            if message == "component exceeded its invocation deadline"
    ));
}

#[tokio::test]
async fn blocking_guest_returns_deadline_exceeded() {
    let (host, plugin) = admitted_fixture().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    let deadline = Duration::from_millis(200);

    let error = guest
        .suspend(
            (),
            CallBudget {
                fuel: CALL_FUEL,
                deadline,
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CallError::DeadlineExceeded { deadline: expired } if expired == deadline
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn compute_bound_guest_returns_deadline_exceeded() {
    let (host, plugin) = admitted_fixture().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    let deadline = Duration::from_millis(50);

    let error = guest
        .spin(
            (),
            CallBudget {
                fuel: COMPUTE_FUEL,
                deadline,
            },
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            CallError::DeadlineExceeded { deadline: expired } if expired == deadline
        ),
        "unexpected call error: {error:?}"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn fast_guest_succeeds_with_a_generous_deadline() {
    let (host, plugin) = admitted_fixture().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    let value = guest
        .value(
            (),
            CallBudget {
                fuel: CALL_FUEL,
                deadline: Duration::from_secs(5),
            },
        )
        .await
        .unwrap();

    assert_eq!(value, 42);
    host.shutdown().await;
}
