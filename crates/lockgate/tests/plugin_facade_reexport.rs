mod common;

use lockgate::PluginId;
use lockgate::{CallError, HostBuilder, PluginConfig, Role, RoleInvocation, RuntimeLimits, Value};

struct ReexportedPluginRole;

struct ReexportedPluginClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for ReexportedPluginRole {
    const INTERFACE: &'static str = "test:reexported-plugin/guest";

    type Budgets = ();

    type Client<'a, S>
        = ReexportedPluginClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        ReexportedPluginClient(invocation)
    }
}

impl ReexportedPluginClient<'_, ()> {
    async fn value(&self) -> Result<u32, CallError> {
        let values = self.0.invoke("value", &[], ()).await?;
        match values.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape("expected one u32 result")),
        }
    }
}

#[tokio::test]
async fn reexport_only_guest_builds_admits_and_invokes() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            PluginId::from("reexported-plugin"),
            &common::REEXPORTED_PLUGIN_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
    let host = builder.finish();

    let guest = host.client::<ReexportedPluginRole>(&plugin).unwrap();
    assert_eq!(guest.value().await.unwrap(), 73);
    host.shutdown().await;
}
