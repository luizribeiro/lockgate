mod common;

use lockgate::{
    Acceptance, CallError, HostBuilder, InvocationCtx, PluginConfig, Role, RoleInvocation,
    RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

const PLUGIN_ID: &str = "config-fixture";

struct SettingsRole;

struct SettingsClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for SettingsRole {
    const INTERFACE: &'static str = "test:config-fixture/guest";

    type Client<'a, S>
        = SettingsClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        SettingsClient(invocation)
    }
}

impl SettingsClient<'_, ()> {
    async fn observed_settings(&self) -> Result<String, CallError> {
        let values = self
            .0
            .invoke("observed-settings", &[], InvocationCtx::bounded(1_000_000))
            .await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }
}

#[tokio::test]
async fn guest_observes_exactly_the_validated_settings_json() {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Config fixture", "1.0").unwrap();
    let component = common::sectioned_fixture(&common::CONFIG_FIXTURE, &metadata);
    let settings = serde_json::json!({ "message": "from the application" });
    let expected = serde_json::to_string(&settings).unwrap();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            PLUGIN_ID,
            &component,
            PluginConfig {
                settings: Some(settings),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    let host = builder.finish();

    let guest = host.client::<SettingsRole>(&plugin).unwrap();
    assert_eq!(guest.observed_settings().await.unwrap(), expected);
}
