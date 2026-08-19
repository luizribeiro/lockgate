mod common;

use std::str::FromStr;

use lockgate::{
    AdmissionError, HostBuilder, InvocationCtx, JsonValueKind, PluginConfig, RuntimeLimits, Scope,
    ScopeError, ScopeReference, ScopeRepr, SymbolicRoots,
};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRef};

const PLUGIN_ID: &str = "com.example.prepared-grants";

#[derive(Clone, Debug, PartialEq, Eq)]
enum TargetScope {
    All,
    Current,
    Pool(String),
    Path(String),
}

impl FromStr for TargetScope {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "all" | "everything" => Ok(Self::All),
            "current" => Ok(Self::Current),
            value
                if value
                    .strip_prefix("pool:")
                    .is_some_and(|name| !name.is_empty()) =>
            {
                Ok(Self::Pool(value[5..].to_owned()))
            }
            value if value.starts_with('/') => Ok(Self::Path(value.to_owned())),
            _ => Err(ScopeError::unknown(value)),
        }
    }
}

impl ScopeRepr for TargetScope {
    fn canonical(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Current => "current".to_owned(),
            Self::Pool(name) => format!("pool:{name}"),
            Self::Path(path) => path.clone(),
        }
    }
}

impl Scope for TargetScope {}

#[lockgate::capability("sessions")]
mod permissions {
    use super::TargetScope;
    use lockgate::{Permission, ScopedPermission};

    pub const READ: ScopedPermission<TargetScope> = ScopedPermission::new("read");
    pub const SEND: Permission = Permission::new("send");
}

#[lockgate::capability("ab")]
mod ab {
    use lockgate::Permission;

    pub const C_X: Permission = Permission::new("c-x");
}

#[lockgate::capability("ab-c")]
mod ab_c {
    use lockgate::Permission;

    pub const X: Permission = Permission::new("x");
}

fn atom(value: &str) -> AtomKey {
    value.parse().unwrap()
}

fn scoped(references: Vec<ScopeRef>) -> NeedEntry {
    NeedEntry::scoped(atom("sessions.read"), references).unwrap()
}

fn required(entry: NeedEntry) -> NeedsManifest {
    NeedsManifest::new(vec![entry], vec![]).unwrap()
}

fn host_builder() -> HostBuilder<()> {
    HostBuilder::new(())
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap()
        .register::<ab::Contract>()
        .unwrap()
        .register::<ab_c::Contract>()
        .unwrap()
}

fn fixture_for(plugin_id: &str, needs: &NeedsManifest) -> Vec<u8> {
    let bytes = common::with_custom_section(
        &common::constant_schema_component(r#"{"type":"object"}"#),
        PLUGIN_METADATA_SECTION,
        &PluginMetadata::new(plugin_id, "Prepared grants fixture", "1.0")
            .unwrap()
            .to_section_bytes()
            .unwrap(),
    );
    common::with_custom_section(
        &bytes,
        PLUGIN_NEEDS_SECTION,
        &needs.to_section_bytes().unwrap(),
    )
}

fn fixture(needs: &NeedsManifest) -> Vec<u8> {
    fixture_for(PLUGIN_ID, needs)
}

#[tokio::test]
async fn literal_setting_and_root_references_prepare_accept_and_admit() {
    let needs = NeedsManifest::new(
        vec![scoped(vec![
            ScopeRef::literal("everything").unwrap(),
            ScopeRef::setting("/scope").unwrap(),
            ScopeRef::root("workspace").unwrap().join("shared").unwrap(),
        ])],
        vec![NeedEntry::flag(atom("sessions.send"))],
    )
    .unwrap();
    let mut roots = SymbolicRoots::default();
    roots.insert("workspace", "/srv/workspace");
    let mut builder = host_builder();
    let prepared = builder
        .prepare(
            PLUGIN_ID,
            &fixture(&needs),
            PluginConfig {
                settings: Some(serde_json::json!({ "scope": "current" })),
                roots,
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();

    let handle = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();

    assert_eq!(handle.id(), PLUGIN_ID);
}

#[tokio::test]
async fn malformed_literal_fails_prepare_with_the_complete_teaching_error() {
    let needs = required(scoped(vec![ScopeRef::literal("gpu").unwrap()]));
    let mut builder = host_builder();

    let error = builder
        .prepare(PLUGIN_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap_err();

    let AdmissionError::ScopeResolution(lockgate::ScopeResolutionError::InvalidScope(detail)) =
        &error
    else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(detail.atom, atom("sessions.read"));
    assert_eq!(detail.reference, ScopeReference::Literal);
    assert_eq!(detail.value, "gpu");
    assert!(detail.scope_type.ends_with("TargetScope"));
    for expected in [
        "sessions.read",
        "literal reference",
        "concrete value `gpu`",
        "TargetScope",
        "unknown scope `gpu`",
    ] {
        assert!(error.to_string().contains(expected));
    }
}

#[tokio::test]
async fn setting_and_root_resolution_failures_are_prepare_errors() {
    let setting = required(scoped(vec![ScopeRef::setting("/scope").unwrap()]));
    for (settings, expected_kind) in [
        (serde_json::json!({ "scope": null }), JsonValueKind::Null),
        (serde_json::json!({ "scope": 7 }), JsonValueKind::Number),
    ] {
        let mut builder = host_builder();
        let error = builder
            .prepare(
                PLUGIN_ID,
                &fixture(&setting),
                PluginConfig {
                    settings: Some(settings),
                    ..PluginConfig::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AdmissionError::ScopeResolution(
                lockgate::ScopeResolutionError::SettingNotString {
                    found,
                    ref pointer,
                    ..
                }
            ) if found == expected_kind && pointer == "/scope"
        ));
    }

    let mut builder = host_builder();
    let error = builder
        .prepare(PLUGIN_ID, &fixture(&setting), PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::ScopeResolution(lockgate::ScopeResolutionError::MissingSetting {
            ref pointer,
            ..
        }) if pointer == "/scope"
    ));

    let root = required(scoped(vec![ScopeRef::root("workspace").unwrap()]));
    let mut builder = host_builder();
    let error = builder
        .prepare(PLUGIN_ID, &fixture(&root), PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::ScopeResolution(lockgate::ScopeResolutionError::UnmappedRoot {
            ref symbol,
            ..
        }) if symbol == "workspace"
    ));
}

#[tokio::test]
async fn acceptance_for_one_plugin_cannot_admit_another() {
    let needs = required(NeedEntry::flag(atom("sessions.send")));
    let mut builder = host_builder();
    let prepared_a = builder
        .prepare(
            "author.plugin-a",
            &fixture_for("author.plugin-a", &needs),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let prepared_b = builder
        .prepare(
            "author.plugin-b",
            &fixture_for("author.plugin-b", &needs),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance_b = prepared_b.accept_all();

    let error = builder
        .admit(
            prepared_a,
            acceptance_b,
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AdmissionError::AcceptancePluginMismatch {
            ref prepared,
            ref acceptance,
        } if prepared == "author.plugin-a" && acceptance == "author.plugin-b"
    ));
    assert!(error.to_string().contains("author.plugin-a"));
    assert!(error.to_string().contains("author.plugin-b"));
}

#[tokio::test]
async fn acceptance_for_stale_settings_resolved_needs_is_rejected() {
    let needs = required(scoped(vec![ScopeRef::setting("/scope").unwrap()]));
    let bytes = fixture(&needs);
    let mut builder = host_builder();
    let stale = builder
        .prepare(
            PLUGIN_ID,
            &bytes,
            PluginConfig {
                settings: Some(serde_json::json!({ "scope": "all" })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let stale_acceptance = stale.accept_all();
    let current = builder
        .prepare(
            PLUGIN_ID,
            &bytes,
            PluginConfig {
                settings: Some(serde_json::json!({ "scope": "current" })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();

    let error = builder
        .admit(
            current,
            stale_acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap_err();

    let AdmissionError::AcceptanceDigestMismatch {
        plugin,
        prepared,
        acceptance,
    } = &error
    else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(plugin, PLUGIN_ID);
    assert_ne!(prepared, acceptance);
    assert!(prepared.starts_with("sha256:"));
    assert!(acceptance.starts_with("sha256:"));
    let message = error.to_string();
    assert!(message.contains(prepared));
    assert!(message.contains(acceptance));
}

#[tokio::test]
async fn dash_prefix_declaration_orders_admit_identically() {
    let entries = || {
        [
            NeedEntry::flag(atom("ab.c-x")),
            NeedEntry::flag(atom("ab-c.x")),
        ]
    };
    let [first, second] = entries();
    let forward = NeedsManifest::new(vec![first, second], vec![]).unwrap();
    let [first, second] = entries();
    let reversed = NeedsManifest::new(vec![second, first], vec![]).unwrap();
    assert_eq!(
        forward.to_section_bytes().unwrap(),
        reversed.to_section_bytes().unwrap()
    );

    let mut builder = host_builder();
    for needs in [&forward, &reversed] {
        let prepared = builder
            .prepare(PLUGIN_ID, &fixture(needs), PluginConfig::default())
            .await
            .unwrap();
        let acceptance = prepared.accept_all();
        let handle = builder
            .admit(
                prepared,
                acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(1_000_000),
            )
            .await
            .unwrap();
        assert_eq!(handle.id(), PLUGIN_ID);
    }
}
