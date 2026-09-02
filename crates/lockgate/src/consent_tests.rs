use std::str::FromStr;

use lockgate_policy::{Need, Needs, Scope, ScopeError, ScopeRef, ScopeRepr, env};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRefEntry};

use crate::policy::diff_grants;
use crate::test_support::{component_from_wit, settings_schema_component, with_section};
use crate::{
    ConsentRecord, ConsentRequired, DriftKind, ExportDrift, ExportDriftKind, GrantReview,
    HostBuilder, PluginConfig, RuntimeLimits, consent_drift,
};

const INSTANCE_ID: &str = "sessions-prod";
const ENV_READ_NEEDS: Needs =
    Needs::required(&[env::READ.need(&[ScopeRef::literal("HOME"), ScopeRef::setting("/scope")])]);

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionScope {
    All,
    Archived,
    Current,
}

impl FromStr for SessionScope {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "all" => Ok(Self::All),
            "archived" => Ok(Self::Archived),
            "current" => Ok(Self::Current),
            _ => Err(ScopeError::unknown(value)),
        }
    }
}

impl ScopeRepr for SessionScope {
    fn canonical(&self) -> String {
        match self {
            Self::All => "all",
            Self::Archived => "archived",
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

fn scoped_need(atom_name: &str, scopes: &[&str]) -> NeedEntry {
    NeedEntry::scoped(
        atom(atom_name),
        scopes
            .iter()
            .map(|scope| ScopeRefEntry::literal(*scope).unwrap())
            .collect(),
    )
    .unwrap()
}

fn config(scope: &str) -> PluginConfig {
    PluginConfig {
        settings: Some(serde_json::json!({ "scope": scope })),
        ..PluginConfig::default()
    }
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
    with_section(
        with_section(
            settings_schema_component(schema),
            PLUGIN_METADATA_SECTION,
            &PluginMetadata::new("author.sessions", "Session helper", "2.0")
                .unwrap()
                .to_section_bytes()
                .unwrap(),
        ),
        PLUGIN_NEEDS_SECTION,
        &needs.to_section_bytes().unwrap(),
    )
}

fn empty_needs_fixture(wit: &str) -> Vec<u8> {
    with_section(
        with_section(
            component_from_wit(wit),
            PLUGIN_METADATA_SECTION,
            &PluginMetadata::new("author.exports", "Export helper", "1.0")
                .unwrap()
                .to_section_bytes()
                .unwrap(),
        ),
        PLUGIN_NEEDS_SECTION,
        &NeedsManifest::empty().to_section_bytes().unwrap(),
    )
}

const ONE_EXPORT: &str = r#"
    package test:consent@1.0.0;

    interface existing {
        ping: func();
    }

    world fixture {
        export existing;
    }
"#;

const TWO_EXPORTS: &str = r#"
    package test:consent@1.0.0;

    interface existing {
        ping: func();
    }

    interface added {
        ping: func();
    }

    world fixture {
        export existing;
        export added;
    }
"#;

const VERSION_ONE_EXPORT: &str = r#"
    package test:versioned@1.0.0;

    interface role {
        ping: func();
    }

    world fixture {
        export role;
    }
"#;

const VERSION_TWO_EXPORT: &str = r#"
    package test:versioned@2.0.0;

    interface role {
        ping: func();
    }

    world fixture {
        export role;
    }
"#;

fn env_read_manifest() -> NeedsManifest {
    use lockgate_policy::__private::{
        need_capability, need_permission, need_scopes, needs_required, scope_ref_wire_byte,
        scope_ref_wire_len,
    };

    let [need]: &[Need] = needs_required(&ENV_READ_NEEDS) else {
        panic!("environment fixture must declare exactly one need")
    };
    let scopes = need_scopes(need)
        .unwrap()
        .iter()
        .map(|reference| {
            let bytes = (0..scope_ref_wire_len(reference))
                .map(|index| scope_ref_wire_byte(reference, index))
                .collect::<Vec<_>>();
            ScopeRefEntry::from_wire(&String::from_utf8(bytes).unwrap()).unwrap()
        })
        .collect();
    let atom = AtomKey::new(need_capability(need), need_permission(need)).unwrap();
    NeedsManifest::new(vec![NeedEntry::scoped(atom, scopes).unwrap()], Vec::new()).unwrap()
}

#[tokio::test]
async fn built_in_env_needs_resolve_names_and_bind_the_request_digest() {
    let needs = env_read_manifest();
    let bytes = fixture(&needs);
    let mut builder = HostBuilder::new(()).unwrap();
    let first = builder
        .prepare(INSTANCE_ID, &bytes, config("LOCKGATE_TOKEN"))
        .await
        .unwrap();
    let second = builder
        .prepare(INSTANCE_ID, &bytes, config("SERVICE_TOKEN"))
        .await
        .unwrap();

    let first_review = first.review();
    let second_review = second.review();
    assert_eq!(first_review.grants.len(), 1);
    assert_eq!(first_review.grants[0].capability, "env");
    assert_eq!(first_review.grants[0].permission, "read");
    assert_eq!(first_review.grants[0].scopes, ["HOME", "LOCKGATE_TOKEN"]);
    assert_eq!(second_review.grants[0].scopes, ["HOME", "SERVICE_TOKEN"]);
    assert_ne!(first_review.request_digest, second_review.request_digest);
}

#[tokio::test]
async fn review_projects_resolved_grants_and_author_reasons() {
    let needs = NeedsManifest::new(
        vec![
            NeedEntry::scoped(
                atom("sessions.read"),
                vec![ScopeRefEntry::setting("/scope").unwrap()],
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
    assert_eq!(review.request_digest, prepared.review().request_digest);
    assert_eq!(review.component_digest, prepared.review().component_digest);
    assert_eq!(review.exported_interfaces, ["lockgate:config/schema"]);
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
                vec![ScopeRefEntry::literal("current").unwrap()],
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
    assert_eq!(record.request_digest, review.request_digest);
    assert_eq!(
        record.component_digest.as_deref(),
        Some(review.component_digest.as_str())
    );
    assert_eq!(record.exported_interfaces, review.exported_interfaces);
    assert_eq!(record.grants, review.grants);
    assert_eq!(decoded, record);
    assert_eq!(decoded.approved_at, "2026-08-19T14:30:00Z");

    let encoded: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(encoded["request_digest"], record.request_digest.to_string());
    assert_eq!(encoded["component_digest"], review.component_digest);
    assert_eq!(
        encoded["exported_interfaces"],
        serde_json::json!(["lockgate:config/schema"])
    );
    assert!(encoded.get("fingerprint").is_none());
}

#[tokio::test]
async fn approval_record_accepts_the_legacy_fingerprint_key() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(INSTANCE_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap();
    let record = prepared.approve("2026-08-19T14:30:00Z".to_owned());
    let legacy_json = serde_json::json!({
        "instance_id": record.instance_id,
        "fingerprint": record.request_digest.to_string(),
        "component_digest": record.component_digest,
        "exported_interfaces": record.exported_interfaces,
        "grants": record.grants,
        "approved_at": record.approved_at,
    });

    let decoded: ConsentRecord = serde_json::from_value(legacy_json).unwrap();

    assert_eq!(decoded, record);
    assert_eq!(decoded.request_digest, record.request_digest);
}

#[tokio::test]
async fn approval_record_without_a_component_digest_deserializes() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(INSTANCE_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap();
    let record = prepared.approve("2026-08-19T14:30:00Z".to_owned());
    let stored_json = serde_json::json!({
        "instance_id": record.instance_id,
        "request_digest": record.request_digest.to_string(),
        "exported_interfaces": record.exported_interfaces,
        "grants": record.grants,
        "approved_at": record.approved_at,
    });

    let decoded: ConsentRecord = serde_json::from_value(stored_json).unwrap();

    assert_eq!(
        decoded,
        ConsentRecord {
            component_digest: None,
            ..record.clone()
        }
    );
    assert_eq!(decoded.component_digest, None);
}

#[tokio::test]
async fn approval_record_without_exported_interfaces_deserializes() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(INSTANCE_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap();
    let record = prepared.approve("2026-08-19T14:30:00Z".to_owned());
    let stored_json = serde_json::json!({
        "instance_id": record.instance_id,
        "request_digest": record.request_digest.to_string(),
        "component_digest": record.component_digest,
        "grants": record.grants,
        "approved_at": record.approved_at,
    });

    let decoded: ConsentRecord = serde_json::from_value(stored_json).unwrap();

    assert!(decoded.exported_interfaces.is_empty());
}

#[tokio::test]
async fn component_digest_changes_do_not_require_new_consent() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let original_bytes = fixture(&needs);
    let rebuilt_bytes = with_section(original_bytes.clone(), "build-id", b"rebuilt");
    let mut prior_builder = builder();
    let prior = prior_builder
        .prepare(INSTANCE_ID, &original_bytes, PluginConfig::default())
        .await
        .unwrap();
    let prior_record = prior.approve("2026-08-19T14:30:00Z".to_owned());

    for component_digest in [None, prior_record.component_digest.clone()] {
        let mut record = prior_record.clone();
        record.component_digest = component_digest;
        let mut current_builder = builder();
        let current = current_builder
            .prepare(INSTANCE_ID, &rebuilt_bytes, PluginConfig::default())
            .await
            .unwrap();

        assert_eq!(record.request_digest, current.review().request_digest);
        if let Some(prior_digest) = &record.component_digest {
            assert_ne!(prior_digest, &current.review().component_digest);
        }

        let acceptance = current.accept_reviewed(Some(&record)).unwrap();
        let admitted = current_builder
            .admit(current, acceptance, RuntimeLimits::default())
            .await
            .unwrap();

        assert_eq!(admitted.id(), INSTANCE_ID);
    }
}

#[tokio::test]
async fn first_run_refuses_acceptance_until_explicit_approval_then_admits() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let mut builder = builder();
    let prepared = builder
        .prepare(INSTANCE_ID, &fixture(&needs), PluginConfig::default())
        .await
        .unwrap();

    let required = prepared.accept_reviewed(None).unwrap_err();
    let ConsentRequired::FirstRun { manifest } = required else {
        panic!("first run must not be reported as drift")
    };
    assert_eq!(manifest, prepared.review());

    let record = prepared.approve("2026-08-19T15:00:00Z".to_owned());
    let acceptance = prepared.accept_reviewed(Some(&record)).unwrap();
    let admitted = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();

    assert_eq!(admitted.id(), INSTANCE_ID);
}

#[tokio::test]
async fn approval_for_another_instance_does_not_authorize_matching_manifest() {
    let needs =
        NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap();
    let bytes = fixture(&needs);
    let mut builder = builder();
    let approved = builder
        .prepare("sessions-staging", &bytes, PluginConfig::default())
        .await
        .unwrap();
    let record = approved.approve("2026-08-19T15:00:00Z".to_owned());
    let current = builder
        .prepare(INSTANCE_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();

    let required = current.accept_reviewed(Some(&record)).unwrap_err();

    assert!(matches!(
        required,
        ConsentRequired::FirstRun { ref manifest } if manifest.instance_id == INSTANCE_ID
    ));
}

#[tokio::test]
async fn same_digest_and_same_exports_are_accepted() {
    let bytes = empty_needs_fixture(ONE_EXPORT);
    let mut prior_builder = HostBuilder::new(()).unwrap();
    let prior = prior_builder
        .prepare(INSTANCE_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let record = prior.approve("2026-08-20T10:00:00Z".to_owned());
    let mut current_builder = HostBuilder::new(()).unwrap();
    let current = current_builder
        .prepare(INSTANCE_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();

    assert_eq!(record.request_digest, current.review().request_digest);
    assert_eq!(
        record.exported_interfaces,
        current.review().exported_interfaces
    );
    current.accept_reviewed(Some(&record)).unwrap();
}

#[tokio::test]
async fn no_needs_approval_does_not_cover_a_new_exported_interface() {
    let mut prior_builder = HostBuilder::new(()).unwrap();
    let prior = prior_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(ONE_EXPORT),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let record = prior.approve("2026-08-20T10:00:00Z".to_owned());
    let mut current_builder = HostBuilder::new(()).unwrap();
    let current = current_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(TWO_EXPORTS),
            PluginConfig::default(),
        )
        .await
        .unwrap();

    assert_eq!(record.request_digest, current.review().request_digest);
    let required = current.accept_reviewed(Some(&record)).unwrap_err();
    assert_eq!(
        required.to_string(),
        "instance `sessions-prod` has expanded its requested access and requires approval"
    );
    let ConsentRequired::Drift { drift, .. } = required else {
        panic!("a gained export must be reported as drift")
    };

    assert!(drift.blocks_admission);
    assert_eq!(drift.changes, []);
    assert!(drift.export_changes.iter().any(|change| {
        change.name == "test:consent/added" && change.kind == ExportDriftKind::Gained
    }));
}

#[tokio::test]
async fn removing_an_exported_interface_is_accepted() {
    let mut prior_builder = HostBuilder::new(()).unwrap();
    let prior = prior_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(TWO_EXPORTS),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let record = prior.approve("2026-08-20T10:00:00Z".to_owned());
    let mut current_builder = HostBuilder::new(()).unwrap();
    let current = current_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(ONE_EXPORT),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let manifest = current.review();
    let drift = consent_drift(&record, &manifest);

    assert_eq!(
        drift.export_changes,
        vec![ExportDrift {
            name: "test:consent/added".to_owned(),
            kind: ExportDriftKind::Lost,
            before: vec!["test:consent/added@1.0.0".to_owned()],
            after: Vec::new(),
        }]
    );
    assert!(!drift.blocks_admission);
    current.accept_reviewed(Some(&record)).unwrap();
}

#[tokio::test]
async fn changing_only_an_exported_interface_version_is_accepted() {
    let mut prior_builder = HostBuilder::new(()).unwrap();
    let prior = prior_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(VERSION_ONE_EXPORT),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let record = prior.approve("2026-08-20T10:00:00Z".to_owned());
    let mut current_builder = HostBuilder::new(()).unwrap();
    let current = current_builder
        .prepare(
            INSTANCE_ID,
            &empty_needs_fixture(VERSION_TWO_EXPORT),
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let manifest = current.review();
    let drift = consent_drift(&record, &manifest);

    assert_eq!(
        drift.export_changes,
        vec![ExportDrift {
            name: "test:versioned/role".to_owned(),
            kind: ExportDriftKind::VersionChanged,
            before: vec!["test:versioned/role@1.0.0".to_owned()],
            after: vec!["test:versioned/role@2.0.0".to_owned()],
        }]
    );
    assert!(!drift.blocks_admission);
    current.accept_reviewed(Some(&record)).unwrap();
}

#[tokio::test]
async fn legacy_record_requires_approval_for_current_exports() {
    let bytes = empty_needs_fixture(ONE_EXPORT);
    let mut builder = HostBuilder::new(()).unwrap();
    let current = builder
        .prepare(INSTANCE_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let mut stored =
        serde_json::to_value(current.approve("2026-08-20T10:00:00Z".to_owned())).unwrap();
    stored
        .as_object_mut()
        .unwrap()
        .remove("exported_interfaces");
    let legacy: ConsentRecord = serde_json::from_value(stored).unwrap();

    assert!(legacy.exported_interfaces.is_empty());
    let required = current.accept_reviewed(Some(&legacy)).unwrap_err();
    let ConsentRequired::Drift { drift, .. } = required else {
        panic!("legacy exports must be reported as drift")
    };

    assert!(drift.blocks_admission);
    assert_eq!(drift.export_changes.len(), 1);
    assert_eq!(drift.export_changes[0].kind, ExportDriftKind::Gained);
    assert_eq!(drift.export_changes[0].name, "test:consent/existing");
}

#[tokio::test]
async fn capability_scope_and_requirement_expansions_refuse_acceptance() {
    let cases = [
        (
            NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap(),
            NeedsManifest::new(
                vec![
                    NeedEntry::flag(atom("sessions.send")),
                    NeedEntry::flag(atom("notify.send")),
                ],
                Vec::new(),
            )
            .unwrap(),
            "notify",
            "send",
            DriftKind::NewGrant,
        ),
        (
            NeedsManifest::new(vec![scoped_need("sessions.read", &["current"])], Vec::new())
                .unwrap(),
            NeedsManifest::new(
                vec![scoped_need("sessions.read", &["current", "all"])],
                Vec::new(),
            )
            .unwrap(),
            "sessions",
            "read",
            DriftKind::ScopeWidened,
        ),
        (
            NeedsManifest::new(Vec::new(), vec![NeedEntry::flag(atom("sessions.send"))]).unwrap(),
            NeedsManifest::new(vec![NeedEntry::flag(atom("sessions.send"))], Vec::new()).unwrap(),
            "sessions",
            "send",
            DriftKind::BecameRequired,
        ),
    ];

    for (before, after, capability, permission, expected_kind) in cases {
        let mut prior_builder = builder();
        let prior = prior_builder
            .prepare(INSTANCE_ID, &fixture(&before), PluginConfig::default())
            .await
            .unwrap();
        let record = prior.approve("2026-08-19T16:00:00Z".to_owned());
        let mut current_builder = builder();
        let current = current_builder
            .prepare(INSTANCE_ID, &fixture(&after), PluginConfig::default())
            .await
            .unwrap();

        let required = current.accept_reviewed(Some(&record)).unwrap_err();
        let ConsentRequired::Drift { manifest, drift } = required else {
            panic!("an expanded request must be reported as drift")
        };

        assert_eq!(manifest, current.review());
        assert!(drift.blocks_admission);
        assert!(drift.changes.iter().any(|change| {
            change.capability == capability
                && change.permission == permission
                && change.kind == expected_kind
        }));
    }
}

#[tokio::test]
async fn config_widening_changes_the_request_digest_and_refuses_acceptance() {
    let needs = NeedsManifest::new(
        vec![
            NeedEntry::scoped(
                atom("sessions.read"),
                vec![
                    ScopeRefEntry::literal("current").unwrap(),
                    ScopeRefEntry::setting("/scope").unwrap(),
                ],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();
    let bytes = fixture(&needs);
    let mut prior_builder = builder();
    let prior = prior_builder
        .prepare(INSTANCE_ID, &bytes, config("current"))
        .await
        .unwrap();
    let record = prior.approve("2026-08-19T16:00:00Z".to_owned());
    let mut current_builder = builder();
    let current = current_builder
        .prepare(INSTANCE_ID, &bytes, config("all"))
        .await
        .unwrap();

    assert_ne!(record.request_digest, current.review().request_digest);
    let required = current.accept_reviewed(Some(&record)).unwrap_err();
    let ConsentRequired::Drift { drift, .. } = required else {
        panic!("config widening must be reported as drift")
    };

    assert!(drift.blocks_admission);
    assert_eq!(drift.changes.len(), 1);
    assert_eq!(drift.changes[0].kind, DriftKind::ScopeWidened);
    assert_eq!(drift.changes[0].before, Some(vec!["current".to_owned()]));
    assert_eq!(
        drift.changes[0].after,
        Some(vec!["all".to_owned(), "current".to_owned()])
    );
}

#[tokio::test]
async fn mixed_scope_addition_and_removal_is_refused_as_widening() {
    let before = NeedsManifest::new(
        vec![scoped_need("sessions.read", &["all", "current"])],
        Vec::new(),
    )
    .unwrap();
    let after = NeedsManifest::new(
        vec![scoped_need("sessions.read", &["archived", "current"])],
        Vec::new(),
    )
    .unwrap();
    let mut prior_builder = builder();
    let prior = prior_builder
        .prepare(INSTANCE_ID, &fixture(&before), PluginConfig::default())
        .await
        .unwrap();
    let record = prior.approve("2026-08-19T16:30:00Z".to_owned());
    let mut current_builder = builder();
    let current = current_builder
        .prepare(INSTANCE_ID, &fixture(&after), PluginConfig::default())
        .await
        .unwrap();

    let required = current.accept_reviewed(Some(&record)).unwrap_err();
    let ConsentRequired::Drift { drift, .. } = required else {
        panic!("a mixed scope change must be reported as drift")
    };

    assert!(drift.blocks_admission);
    assert_eq!(drift.changes.len(), 1);
    assert_eq!(drift.changes[0].kind, DriftKind::ScopeWidened);
    assert_eq!(
        drift.changes[0].before,
        Some(vec!["all".to_owned(), "current".to_owned()])
    );
    assert_eq!(
        drift.changes[0].after,
        Some(vec!["archived".to_owned(), "current".to_owned()])
    );
}

#[tokio::test]
async fn scope_and_requirement_narrowing_rebinds_to_the_current_manifest() {
    let before = NeedsManifest::new(
        vec![
            scoped_need("sessions.read", &["all", "current"]),
            NeedEntry::flag(atom("sessions.send")),
        ],
        Vec::new(),
    )
    .unwrap();
    let after = NeedsManifest::new(
        vec![scoped_need("sessions.read", &["current"])],
        vec![NeedEntry::flag(atom("sessions.send"))],
    )
    .unwrap();
    let mut prior_builder = builder();
    let prior = prior_builder
        .prepare(INSTANCE_ID, &fixture(&before), PluginConfig::default())
        .await
        .unwrap();
    let record = prior.approve("2026-08-19T16:00:00Z".to_owned());
    let mut current_builder = builder();
    let current = current_builder
        .prepare(INSTANCE_ID, &fixture(&after), PluginConfig::default())
        .await
        .unwrap();
    let current_review = current.review();
    let drift = consent_drift(&record, &current_review);

    assert!(!drift.blocks_admission);
    assert_eq!(
        drift
            .changes
            .iter()
            .map(|change| change.kind)
            .collect::<Vec<_>>(),
        vec![DriftKind::ScopeNarrowed, DriftKind::BecameOptional]
    );

    let acceptance = current.accept_reviewed(Some(&record)).unwrap();
    let admitted = current_builder
        .admit(current, acceptance, RuntimeLimits::default())
        .await
        .unwrap();

    assert_eq!(
        admitted
            .effective_grants()
            .scoped_values(&atom("sessions.read")),
        Some(["current".to_owned()].as_slice())
    );
}

#[test]
fn drift_classifies_every_change_and_blocks_if_and_only_if_an_expansion_exists() {
    let grant =
        |capability: &str, permission: &str, scopes: &[&str], optional: bool| -> GrantReview {
            GrantReview {
                capability: capability.to_owned(),
                permission: permission.to_owned(),
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
                optional,
                reason: None,
            }
        };
    let before = vec![
        grant("cache", "read", &[], false),
        grant("files", "read", &["data", "workspace"], false),
        grant("sessions", "read", &["current"], false),
        grant("state", "write", &[], false),
        grant("vm", "exec", &["gpu"], true),
    ];
    let after = vec![
        grant("cache", "read", &[], true),
        grant("files", "read", &["data"], false),
        grant("notify", "send", &[], true),
        grant("sessions", "read", &["all", "current"], false),
        grant("vm", "exec", &["gpu"], false),
    ];

    let expanded = diff_grants(&before, &after);

    assert_eq!(
        expanded
            .changes
            .iter()
            .map(|change| (&*change.capability, change.kind))
            .collect::<Vec<_>>(),
        vec![
            ("cache", DriftKind::BecameOptional),
            ("files", DriftKind::ScopeNarrowed),
            ("notify", DriftKind::NewGrant),
            ("sessions", DriftKind::ScopeWidened),
            ("state", DriftKind::RemovedGrant),
            ("vm", DriftKind::BecameRequired),
        ]
    );
    assert!(expanded.blocks_admission);

    let narrowing_only = diff_grants(
        &before[..4],
        &[
            grant("cache", "read", &[], true),
            grant("files", "read", &["data"], false),
            grant("sessions", "read", &["current"], false),
        ],
    );
    assert_eq!(
        narrowing_only
            .changes
            .iter()
            .map(|change| change.kind)
            .collect::<Vec<_>>(),
        vec![
            DriftKind::BecameOptional,
            DriftKind::ScopeNarrowed,
            DriftKind::RemovedGrant,
        ]
    );
    assert!(!narrowing_only.blocks_admission);

    let optional_but_wider = diff_grants(
        &[grant("sessions", "read", &["current"], false)],
        &[grant("sessions", "read", &["all", "current"], true)],
    );
    assert_eq!(optional_but_wider.changes[0].kind, DriftKind::ScopeWidened);
    assert!(optional_but_wider.blocks_admission);
}
