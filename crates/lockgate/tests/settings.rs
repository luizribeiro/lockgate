mod common;

use lockgate::{
    Acceptance, CallError, HostBuilder, InvocationCtx, PluginConfig, Role, RoleInvocation,
    RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

const PLUGIN_ID: &str = "config-fixture";

struct SettingsRole;
struct TypedSettingsRole;

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

impl Role for TypedSettingsRole {
    const INTERFACE: &'static str = "test:typed-settings/guest";

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

    async fn typed_observed_settings(&self) -> Result<String, CallError> {
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

#[tokio::test]
async fn typed_guest_settings_round_trip_and_apply_serde_defaults() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "typed-settings",
            &common::TYPED_SETTINGS_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "required": "from-host" })),
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

    let guest = host.client::<TypedSettingsRole>(&plugin).unwrap();
    assert_eq!(
        guest.typed_observed_settings().await.unwrap(),
        "from-host:guest-default"
    );
}

#[tokio::test]
async fn facade_no_settings_accepts_absent_and_empty_configuration() {
    for settings in [None, Some(serde_json::json!({}))] {
        let mut builder = HostBuilder::new(()).unwrap();
        builder
            .prepare(
                "greeter",
                &common::PUBLIC_FIXTURE,
                PluginConfig {
                    settings,
                    ..PluginConfig::default()
                },
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn facade_no_settings_rejects_every_supplied_key_by_name() {
    let mut builder = HostBuilder::new(()).unwrap();
    let error = builder
        .prepare(
            "greeter",
            &common::PUBLIC_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "surprise": true })),
                ..PluginConfig::default()
            },
        )
        .await
        .err()
        .unwrap();

    assert!(error.to_string().contains("surprise"), "{error}");
}
