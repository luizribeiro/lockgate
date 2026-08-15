mod common;

use lockgate::{
    Acceptance, AdmissionError, BudgetClass, HostBuilder, InspectError, InvocationCtx, LimitSet,
    PluginConfig, Role, RoleError, RoleInvocation, RuntimeLimits, SymbolicRoots, inspect,
};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, GrantSet, NeedEntry, NeedsDigest, NeedsManifest, PluginMetadata};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const PLUGIN_ID: &str = "com.example.lifecycle";

struct GuestRole;

impl Role for GuestRole {
    const INTERFACE: &'static str = "test:lifecycle/guest";
    type Client<'a, S>
        = RoleInvocation<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        invocation
    }
}

struct MissingRole;

impl Role for MissingRole {
    const INTERFACE: &'static str = "test:lifecycle/missing";
    type Client<'a, S>
        = RoleInvocation<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        invocation
    }
}

fn component(wit: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.select_world(&[package], None).unwrap();
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

fn value_component() -> Vec<u8> {
    component(
        "package test:lifecycle; interface guest { value: func() -> u32; } world fixture { export guest; }",
    )
}

fn engine_rejected_component() -> Vec<u8> {
    component(
        "package test:engine-rejected; interface guest { type failure = error-context; run: func() -> option<failure>; } world fixture { export guest; }",
    )
}

fn memory_growing_component() -> Vec<u8> {
    let module = wat::parse_str(
        "(module (memory 0) (func $start (drop (memory.grow (i32.const 1)))) (start $start))",
    )
    .unwrap();
    let mut component = wasm_encoder::ComponentBuilder::default();
    let module = component.core_module_raw(None, &module);
    component.core_instantiate(
        None,
        module,
        std::iter::empty::<(&str, wasm_encoder::ModuleArg)>(),
    );
    component.finish()
}

fn component_with_unwired_import() -> Vec<u8> {
    component(
        "package test:unwired; interface host { wait: func(); } interface guest { value: func() -> u32; } world fixture { import host; export guest; }",
    )
}

fn metadata() -> PluginMetadata {
    PluginMetadata::new(PLUGIN_ID, "Lifecycle fixture", "1.0").unwrap()
}

fn well_formed_fixture() -> Vec<u8> {
    common::sectioned_fixture(&value_component(), &metadata())
}

fn fixture_with_needs(needs: &NeedsManifest) -> Vec<u8> {
    let bytes = common::with_custom_section(
        &value_component(),
        PLUGIN_METADATA_SECTION,
        &metadata().to_section_bytes().unwrap(),
    );
    common::with_custom_section(
        &bytes,
        PLUGIN_NEEDS_SECTION,
        &needs.to_section_bytes().unwrap(),
    )
}

#[test]
fn runtime_inputs_are_bounded_and_explicit() {
    let limits = RuntimeLimits::default();
    assert!(limits.instantiation_fuel > 0);
    assert!(limits.instantiation_fuel < u64::MAX);
    assert!(limits.max_memory_bytes > 0);
    assert!(limits.max_memory_bytes < usize::MAX);

    assert_eq!(
        InvocationCtx::bounded(123),
        InvocationCtx::new((), BudgetClass::Bounded { fuel: 123 })
    );
    assert_eq!(
        InvocationCtx::new("startup", BudgetClass::Bounded { fuel: 456 }).data,
        "startup"
    );
}

#[tokio::test]
async fn empty_needs_accept_both_consent_paths_and_ignore_stale_grants() {
    let bytes = well_formed_fixture();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let handle = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    assert_eq!(handle.id(), PLUGIN_ID);
    assert_eq!(handle.metadata(), &metadata());

    let mut stale = GrantSet::new();
    stale.insert_flag("obsolete.feature".parse().unwrap());
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let handle = builder
        .admit(
            prepared,
            Acceptance::accepted(stale),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    assert_eq!(handle.id(), PLUGIN_ID);
}

#[tokio::test]
async fn three_verb_lifecycle_finishes_with_the_admitted_plugin() {
    let bytes = well_formed_fixture();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let admitted = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();

    let host = builder.finish();
    let plugins: Vec<_> = host.plugins().collect();
    assert_eq!(plugins, [&admitted]);
    assert_eq!(plugins[0].id(), PLUGIN_ID);
    assert_eq!(plugins[0].metadata(), &metadata());
}

#[tokio::test]
async fn role_casts_fail_before_calling_for_missing_roles_and_wrong_hosts() {
    let bytes = well_formed_fixture();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let handle = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    let host = builder.finish();

    host.client::<GuestRole>(&handle).unwrap();
    let error = host.client::<MissingRole>(&handle).unwrap_err();
    assert!(matches!(
        error,
        RoleError::RoleNotExported {
            interface: "test:lifecycle/missing"
        }
    ));
    assert!(error.to_string().contains("test:lifecycle/missing"));

    let other_host = HostBuilder::new(()).unwrap().finish();
    let error = other_host.client::<GuestRole>(&handle).unwrap_err();
    assert!(matches!(error, RoleError::WrongHost));
    assert!(error.to_string().contains("different Lockgate Host"));
}

#[tokio::test]
async fn unregistered_declared_capability_fails_before_smoke() {
    let atom: AtomKey = "http.request".parse().unwrap();
    let needs = NeedsManifest::new(vec![NeedEntry::flag(atom.clone())], vec![]).unwrap();
    let bytes = fixture_with_needs(&needs);
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();

    let error = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(0),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::UnregisteredCapability { atom: ref found } if found == &atom
    ));
    assert!(error.to_string().contains("http.request"));
    assert!(error.to_string().contains("application never registered"));
}

#[tokio::test]
async fn smoke_instantiation_budget_exhaustion_is_typed() {
    let bytes = well_formed_fixture();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let limits = RuntimeLimits {
        instantiation_fuel: 0,
        ..RuntimeLimits::default()
    };

    let error = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            limits,
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::SmokeOutOfBudget));
    assert!(error.to_string().contains("startup budget"));
}

#[tokio::test]
async fn smoke_instantiation_applies_the_store_memory_cap() {
    let bytes = common::sectioned_fixture(&memory_growing_component(), &metadata());
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let limits = RuntimeLimits {
        max_memory_bytes: 0,
        ..RuntimeLimits::default()
    };

    let error = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            limits,
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::SmokeFailure { .. }));
    assert!(error.to_string().contains("linear memory growth"));
    assert!(error.to_string().contains("0-byte limit"));
}

#[tokio::test]
async fn prepares_a_well_formed_sectioned_fixture() {
    let bytes = well_formed_fixture();
    let free = inspect(&bytes).unwrap();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();

    assert_eq!(prepared.inspection(), &free);
    assert_eq!(free.metadata(), &metadata());
    assert_eq!(free.needs(), &NeedsManifest::empty());
    assert_eq!(
        free.needs_digest(),
        NeedsDigest::compute(&NeedsManifest::empty()).unwrap()
    );
    assert_eq!(free.exported_interfaces(), ["test:lifecycle/guest"]);
}

#[tokio::test]
async fn preparation_reports_each_pre_compilation_failure() {
    let base = value_component();
    let needs = NeedsManifest::empty().to_section_bytes().unwrap();
    let metadata_bytes = metadata().to_section_bytes().unwrap();
    let missing_metadata =
        common::with_custom_section(&base, PLUGIN_NEEDS_SECTION, needs.as_slice());
    let missing_needs =
        common::with_custom_section(&base, PLUGIN_METADATA_SECTION, &metadata_bytes);
    let malformed_needs = common::with_custom_section(
        &common::with_custom_section(&base, PLUGIN_METADATA_SECTION, &metadata_bytes),
        PLUGIN_NEEDS_SECTION,
        b"not JSON",
    );
    let well_formed = well_formed_fixture();
    let mut builder = HostBuilder::new(()).unwrap();

    let error = builder
        .prepare(PLUGIN_ID, &missing_metadata, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::Inspection(InspectError::MissingMetadata)
    ));
    assert!(
        error
            .to_string()
            .contains("missing required `lockgate:plugin`")
    );

    let error = builder
        .prepare(PLUGIN_ID, &missing_needs, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::Inspection(InspectError::MissingNeeds)
    ));
    assert!(
        error
            .to_string()
            .contains("missing required `lockgate:needs`")
    );

    let configured_id = "com.example.other";
    let error = builder
        .prepare(configured_id, &well_formed, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::PluginIdMismatch { .. }));
    assert!(error.to_string().contains(PLUGIN_ID));
    assert!(error.to_string().contains(configured_id));

    let error = builder
        .prepare(PLUGIN_ID, &malformed_needs, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::Inspection(InspectError::Needs(_))
    ));
    assert!(error.to_string().contains("needs manifest is invalid"));

    let error = builder
        .prepare(PLUGIN_ID, b"not a component", PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::Inspection(InspectError::InvalidComponent { .. })
    ));
    assert!(
        error
            .to_string()
            .contains("not a valid WebAssembly component")
    );
}

#[tokio::test]
async fn forbidden_exports_keep_the_stable_teaching_error() {
    let bytes = common::sectioned_fixture(&engine_rejected_component(), &metadata());

    // The fixture fails Wasmtime compilation, so receiving the stable teaching
    // error below explicitly guards validation-before-compilation ordering.
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model(true)
        .wasm_component_model_async(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    assert!(wasmtime::component::Component::new(&engine, &bytes).is_err());

    let mut builder = HostBuilder::new(()).unwrap();
    let error = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap_err();

    assert!(matches!(error, AdmissionError::UnsupportedExport(_)));
    assert_eq!(error.code(), Some("admission.unsupported-export"));
    assert!(
        error
            .to_string()
            .starts_with("[admission.unsupported-export] ")
    );
    assert!(error.to_string().contains("offending type `error-context`"));
}

#[tokio::test]
async fn validator_passing_unwired_import_fails_linker_preflight() {
    let bytes = common::sectioned_fixture(&component_with_unwired_import(), &metadata());
    let mut builder = HostBuilder::new(()).unwrap();
    let error = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap_err();

    assert!(matches!(error, AdmissionError::Preflight { .. }));
    assert!(error.to_string().contains("linker preflight failed"));
}

#[tokio::test]
async fn non_default_config_is_rejected_instead_of_ignored() {
    let mut builder = HostBuilder::new(()).unwrap();
    let mut roots = SymbolicRoots::default();
    roots.insert("workspace", "/tmp/workspace");
    let cases = [
        (
            PluginConfig {
                settings: Some(serde_json::json!({ "enabled": true })),
                ..PluginConfig::default()
            },
            "settings",
        ),
        (
            PluginConfig {
                roots,
                ..PluginConfig::default()
            },
            "symbolic roots",
        ),
        (
            PluginConfig {
                limits: LimitSet::Constrained,
                ..PluginConfig::default()
            },
            "grant limits",
        ),
    ];

    for (config, field) in cases {
        let error = builder
            .prepare(PLUGIN_ID, &well_formed_fixture(), config)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AdmissionError::ConfigFeatureUnavailable { field: found } if found == field
        ));
        assert!(
            error
                .to_string()
                .contains("not yet available in this build")
        );
    }
}

#[tokio::test]
async fn artifact_validation_precedes_temporary_config_rejection() {
    let mut builder = HostBuilder::new(()).unwrap();
    let config = PluginConfig {
        settings: Some(serde_json::json!({ "enabled": true })),
        ..PluginConfig::default()
    };

    let error = builder
        .prepare(PLUGIN_ID, &value_component(), config)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::Inspection(InspectError::MissingMetadata)
    ));
}
