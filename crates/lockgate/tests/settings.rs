mod common;

use lockgate::PluginId;
use lockgate::{
    AdmissionError, CallError, HostBuilder, PluginConfig, Role, RoleInvocation, RuntimeLimits,
    Value,
};
use lockgate_schema::PluginMetadata;

const PLUGIN_ID: &str = "config-fixture";

struct SettingsRole;
struct TypedSettingsRole;

struct SettingsClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for SettingsRole {
    const INTERFACE: &'static str = "test:config-fixture/guest";

    type Budgets = ();

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

    type Budgets = ();

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
        let values = self.0.invoke("observed-settings", &[], ()).await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }

    async fn typed_observed_settings(&self) -> Result<String, CallError> {
        let values = self.0.invoke("observed-settings", &[], ()).await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }
}

#[tokio::test]
async fn valid_config_preflights_without_acceptance_and_remains_admissible() {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Config fixture", "1.0").unwrap();
    let component = common::sectioned_fixture(&common::CONFIG_FIXTURE, &metadata);
    let settings = serde_json::json!({ "message": "from the application" });
    let expected = serde_json::to_string(&settings).unwrap();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            PluginId::from(PLUGIN_ID),
            &component,
            PluginConfig {
                settings: Some(settings),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let preflight = builder
        .preflight(&prepared, &RuntimeLimits::default())
        .await
        .unwrap();
    assert!(preflight.required_environment_variables.is_empty());
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
    let host = builder.finish();
    assert_eq!(host.plugins().count(), 1);

    let guest = host.client::<SettingsRole>(&plugin).unwrap();
    assert_eq!(guest.observed_settings().await.unwrap(), expected);
    host.shutdown().await;
}

#[tokio::test]
async fn typed_guest_settings_round_trip_and_apply_serde_defaults() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            PluginId::from("typed-settings"),
            &common::TYPED_SETTINGS_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "required": "from-host" })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
    let host = builder.finish();

    let guest = host.client::<TypedSettingsRole>(&plugin).unwrap();
    assert_eq!(
        guest.typed_observed_settings().await.unwrap(),
        "from-host:guest-default"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn invalid_settings_fail_preflight_and_admission_identically() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            PluginId::from("typed-settings"),
            &common::TYPED_SETTINGS_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let preflight_error = builder
        .preflight(&prepared, &RuntimeLimits::default())
        .await
        .unwrap_err();
    let acceptance = prepared.accept_all();
    let admission_error = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap_err();

    assert!(matches!(
        preflight_error,
        AdmissionError::SettingsValidation { .. }
    ));
    assert!(matches!(
        admission_error,
        AdmissionError::SettingsValidation { .. }
    ));
    assert_eq!(preflight_error.to_string(), admission_error.to_string());
    assert!(
        preflight_error
            .to_string()
            .contains("plugin settings do not match their schema")
    );
    assert!(preflight_error.to_string().contains("required"));
}

#[tokio::test]
async fn facade_no_settings_accepts_absent_and_empty_configuration() {
    for settings in [None, Some(serde_json::json!({}))] {
        let mut builder = HostBuilder::new(()).unwrap();
        builder
            .prepare(
                PluginId::from("greeter"),
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
    let prepared = builder
        .prepare(
            PluginId::from("greeter"),
            &common::PUBLIC_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "surprise": true })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let error = builder
        .preflight(&prepared, &RuntimeLimits::default())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("surprise"), "{error}");
}
