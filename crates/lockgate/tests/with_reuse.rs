mod common;

use lockgate::{
    CallError, HostBuilder, InvocationCtx, PluginConfig, Role, RoleInvocation, RuntimeLimits, Value,
};

struct WithReuseRole;

struct WithReuseClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for WithReuseRole {
    const INTERFACE: &'static str = "test:with-reuse/guest@1.2.3";

    type Client<'a, S>
        = WithReuseClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        WithReuseClient(invocation)
    }
}

impl WithReuseClient<'_, ()> {
    async fn round_trip(&self, value: u32) -> Result<u32, CallError> {
        let arguments = [Value::Record(vec![("value".into(), Value::U32(value))])];
        let values = self
            .0
            .invoke(
                "round-trip",
                &arguments,
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            )
            .await?;
        match values.as_slice() {
            [Value::Record(fields)] => match fields.as_slice() {
                [(name, Value::U32(value))] if name == "value" => Ok(*value),
                _ => Err(CallError::shape("expected a request record result")),
            },
            _ => Err(CallError::shape("expected one record result")),
        }
    }
}

#[tokio::test]
async fn mapped_shared_type_builds_admits_and_round_trips() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "with-reuse",
            &common::WITH_REUSE_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
        )
        .await
        .unwrap();
    let host = builder.finish();

    let guest = host.client::<WithReuseRole>(&plugin).unwrap();
    assert_eq!(guest.round_trip(42).await.unwrap(), 42);
}
