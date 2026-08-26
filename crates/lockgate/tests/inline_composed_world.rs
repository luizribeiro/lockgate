mod common;

use lockgate::{
    CallError, HostBuilder, InvocationCtx, PluginConfig, Role, RoleInvocation, RuntimeLimits, Value,
};

struct InlineComposedRole;

struct InlineComposedClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for InlineComposedRole {
    const INTERFACE: &'static str = "test:inline-composed/guest";

    type Client<'a, S>
        = InlineComposedClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        InlineComposedClient(invocation)
    }
}

impl InlineComposedClient<'_, ()> {
    async fn run(&self) -> Result<u32, CallError> {
        let values = self
            .0
            .invoke(
                "run",
                &[],
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            )
            .await?;
        match values.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape("expected one u32 result")),
        }
    }
}

#[tokio::test]
async fn inline_composed_world_builds_admits_and_invokes() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "inline-composed",
            &common::INLINE_COMPOSED_FIXTURE,
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

    let guest = host.client::<InlineComposedRole>(&plugin).unwrap();
    assert_eq!(guest.run().await.unwrap(), 42);
}
