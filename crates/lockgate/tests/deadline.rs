mod common;

use std::future::pending;
use std::time::Duration;

use lockgate::__private::{StoreCtx, wasmtime};
use lockgate::{
    AdmissionError, CallError, Host, HostBuilder, HostImports, InvocationCtx, PluginConfig,
    PluginHandle, Role, RoleInvocation, RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

const CALL_FUEL: u64 = 1_000_000;

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
    async fn value(&self, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        self.call_u32("value", ctx).await
    }

    async fn suspend(&self, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        self.call_u32("suspend", ctx).await
    }

    async fn call_u32(&self, function: &str, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        let results = self.0.invoke(function, &[], ctx).await?;
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
        .prepare("deadline", &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(CALL_FUEL),
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
    let mut builder = HostBuilder::new(BlockingImports).unwrap();
    let metadata = PluginMetadata::new("deadline-start", "Deadline start fixture", "1.0").unwrap();
    let bytes = common::sectioned_fixture(&blocking_start_component(), &metadata);
    let prepared = builder
        .prepare("deadline-start", &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let deadline = Duration::from_millis(200);

    let error = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded_with_deadline(CALL_FUEL, deadline),
        )
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
        .suspend(InvocationCtx::bounded_with_deadline(CALL_FUEL, deadline))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CallError::DeadlineExceeded { deadline: expired } if expired == deadline
    ));
}

#[tokio::test]
async fn fast_guest_succeeds_with_a_generous_deadline() {
    let (host, plugin) = admitted_fixture().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    let value = guest
        .value(InvocationCtx::bounded_with_deadline(
            CALL_FUEL,
            Duration::from_secs(5),
        ))
        .await
        .unwrap();

    assert_eq!(value, 42);
}

#[tokio::test]
async fn fast_guest_succeeds_without_a_deadline() {
    let (host, plugin) = admitted_fixture().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();

    let value = guest
        .value(InvocationCtx::bounded(CALL_FUEL))
        .await
        .unwrap();

    assert_eq!(value, 42);
}
