// This test is the product's minimal honest usage and MUST stay green through every
// future step; changing it means the public contract moved.

mod common;

use lockgate::{CallError, HostBuilder, PluginConfig, Role, RoleInvocation, RuntimeLimits, Value};
use lockgate_schema::PluginMetadata;

const PLUGIN_ID: &str = "greeter";
const GREETER_INTERFACE: &str = "test:public/greeter";

struct GreeterRole;

struct GreeterClient<'a, S: Send + Sync + 'static> {
    invocation: RoleInvocation<'a, S>,
}

impl Role for GreeterRole {
    const INTERFACE: &'static str = GREETER_INTERFACE;

    type Budgets = ();

    type Client<'a, S>
        = GreeterClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        GreeterClient { invocation }
    }
}

impl GreeterClient<'_, ()> {
    async fn greet(&self, name: &str) -> Result<String, CallError> {
        let results = self
            .invocation
            .invoke("greet", &[Value::String(name.to_owned())], ())
            .await?;
        match results.as_slice() {
            [Value::String(greeting)] => Ok(greeting.clone()),
            _ => Err(CallError::shape("greeter returned a non-string result")),
        }
    }
}

#[tokio::test]
async fn minimal_embedder_calls_a_greeter() {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Greeter", "1.0").unwrap();
    let bytes = common::sectioned_fixture(&common::PUBLIC_FIXTURE, &metadata);

    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
    let host = builder.finish();

    let greeter = host.client::<GreeterRole>(&plugin).unwrap();
    let output = greeter.greet("world").await.unwrap();
    assert_eq!(output, "Hello, world!");
    host.shutdown().await;
}
