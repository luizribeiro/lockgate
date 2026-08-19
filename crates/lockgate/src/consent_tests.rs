use std::str::FromStr;

use lockgate_policy::{Scope, ScopeError, ScopeRepr};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRef};
use wasm_encoder::{ComponentSection, CustomSection};

use crate::{ConsentRecord, HostBuilder, PluginConfig};

const INSTANCE_ID: &str = "sessions-prod";

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionScope {
    All,
    Current,
}

impl FromStr for SessionScope {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "all" => Ok(Self::All),
            "current" => Ok(Self::Current),
            _ => Err(ScopeError::unknown(value)),
        }
    }
}

impl ScopeRepr for SessionScope {
    fn canonical(&self) -> String {
        match self {
            Self::All => "all",
            Self::Current => "current",
        }
        .to_owned()
    }
}

impl Scope for SessionScope {}

#[lockgate_policy::capability("sessions")]
mod sessions {
    use super::SessionScope;
    use lockgate_policy::{Permission, ScopedPermission};

    pub const READ: ScopedPermission<SessionScope> = ScopedPermission::new("read");
    pub const SEND: Permission = Permission::new("send");
}

#[lockgate_policy::capability("notify")]
mod notify {
    use lockgate_policy::Permission;

    pub const SEND: Permission = Permission::new("send");
}

fn atom(value: &str) -> AtomKey {
    value.parse().unwrap()
}

fn builder() -> HostBuilder<()> {
    HostBuilder::new(())
        .unwrap()
        .register::<sessions::Contract>()
        .unwrap()
        .register::<notify::Contract>()
        .unwrap()
}

fn fixture(needs: &NeedsManifest) -> Vec<u8> {
    let schema = r#"{"type":"object","properties":{"scope":{"type":"string"}},"additionalProperties":false}"#;
    let encoded = schema
        .as_bytes()
        .iter()
        .map(|byte| format!(r"\{byte:02x}"))
        .collect::<String>();
    let mut bytes = wat::parse_str(format!(
        r#"(component
            (core module $guest
                (memory (export "memory") 1)
                (data (i32.const 64) "{encoded}")
                (func (export "settings-schema") (result i32)
                    (i32.store (i32.const 8) (i32.const 64))
                    (i32.store offset=4 (i32.const 8) (i32.const {length}))
                    (i32.const 8)
                )
            )
            (core instance $guest-instance (instantiate $guest))
            (func $settings-schema (result string)
                (canon lift
                    (core func $guest-instance "settings-schema")
                    (memory (core memory $guest-instance "memory"))
                )
            )
            (instance $schema
                (export "settings-schema" (func $settings-schema))
            )
            (export "lockgate:config/schema" (instance $schema))
        )"#,
        length = schema.len(),
    ))
    .unwrap();
    for (name, data) in [
        (
            PLUGIN_METADATA_SECTION,
            PluginMetadata::new("author.sessions", "Session helper", "2.0")
                .unwrap()
                .to_section_bytes()
                .unwrap(),
        ),
        (PLUGIN_NEEDS_SECTION, needs.to_section_bytes().unwrap()),
    ] {
        CustomSection {
            name: name.into(),
            data: data.into(),
        }
        .append_to_component(&mut bytes);
    }
    bytes
}

#[tokio::test]
async fn review_projects_resolved_grants_and_author_reasons() {
    let needs = NeedsManifest::new(
        vec![
            NeedEntry::scoped(
                atom("sessions.read"),
                vec![ScopeRef::setting("/scope").unwrap()],
            )
            .unwrap()
            .with_reason("Read the selected sessions")
            .unwrap(),
        ],
        vec![
            NeedEntry::flag(atom("notify.send"))
                .with_reason("Send a completion notice")
                .unwrap(),
        ],
    )
    .unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(
            INSTANCE_ID,
            &fixture(&needs),
            PluginConfig {
                settings: Some(serde_json::json!({ "scope": "current" })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();

    let review = prepared.review();

    assert_eq!(review.instance_id, INSTANCE_ID);
    assert_eq!(review.plugin_label, "Session helper");
    assert_eq!(review.fingerprint, prepared.review().fingerprint);
    assert_eq!(review.grants.len(), 2);
    assert_eq!(review.grants[0].capability, "notify");
    assert_eq!(review.grants[0].permission, "send");
    assert!(review.grants[0].scopes.is_empty());
    assert!(review.grants[0].optional);
    assert_eq!(
        review.grants[0].reason.as_deref(),
        Some("Send a completion notice")
    );
    assert_eq!(review.grants[1].capability, "sessions");
    assert_eq!(review.grants[1].permission, "read");
    assert_eq!(review.grants[1].scopes, ["current"]);
    assert!(!review.grants[1].optional);
    assert_eq!(
        review.grants[1].reason.as_deref(),
        Some("Read the selected sessions")
    );
}

#[tokio::test]
async fn approval_record_round_trip_preserves_the_complete_drift_basis() {
    let needs = NeedsManifest::new(
        vec![NeedEntry::flag(atom("sessions.send"))],
        vec![
            NeedEntry::scoped(
                atom("sessions.read"),
                vec![ScopeRef::literal("current").unwrap()],
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(INSTANCE_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap();
    let review = prepared.review();

    let record = prepared.approve("2026-08-19T14:30:00Z".to_owned());
    let encoded = serde_json::to_vec(&record).unwrap();
    let decoded: ConsentRecord = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(record.instance_id, review.instance_id);
    assert_eq!(record.fingerprint, review.fingerprint);
    assert_eq!(record.grants, review.grants);
    assert_eq!(decoded, record);
    assert_eq!(decoded.approved_at, "2026-08-19T14:30:00Z");
}
