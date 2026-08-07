//! Cross-layer tests for discovery, planning, registry authority, and fuel isolation.
//! Small synthesized components keep the library suite independent from the runnable demo.

use crate::{Catalog, Plan, PlanError, Policy, Runtime, RuntimeError};
use std::error::Error as _;
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

fn component_bytes(wit: &str, world_name: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.packages[package].worlds[world_name];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

#[test]
fn catalog_rejects_fixed_lists_before_runtime_type_introspection() {
    let wit = r#"package demo:fixed-list@0.1.0;

interface api { run: func(input: list<u32, 4>); }
world caller { import api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let error = catalog
        .add("caller", component_bytes(wit, "caller"))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not a valid supported component")
    );
    assert!(
        error
            .source()
            .unwrap()
            .to_string()
            .contains("fixed-length lists")
    );
}

#[test]
fn catalog_rejects_cross_store_resource_imports() {
    let wit = r#"package demo:resources@0.1.0;

interface api {
  resource file;
  run: func(input: borrow<file>);
}
world caller { import api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let error = catalog
        .add("caller", component_bytes(wit, "caller"))
        .unwrap_err();
    assert!(error.source().unwrap().to_string().contains("resource"));
}

#[test]
fn plan_requires_an_explicit_link_even_when_the_provider_is_present() {
    let wit = r#"package demo:missing@0.1.0;
interface api { run: func(); }
world caller { import api; }
world provider { export api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let caller = catalog
        .add("caller", component_bytes(wit, "caller"))
        .unwrap();
    let provider = catalog
        .add("provider", component_bytes(wit, "provider"))
        .unwrap();
    let policy = Policy::builder(&catalog)
        .include(caller)
        .unwrap()
        .include(provider)
        .unwrap()
        .build();
    assert!(matches!(
        Plan::new(catalog, policy),
        Err(PlanError::MissingProvider { caller, interface })
            if caller == "caller" && interface == "demo:missing/api@0.1.0"
    ));
}

#[test]
fn plan_rejects_ambiguous_providers() {
    let wit = r#"package demo:ambiguous@0.1.0;
interface api { run: func(); }
world caller { import api; }
world provider { export api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let caller = catalog
        .add("caller", component_bytes(wit, "caller"))
        .unwrap();
    let first = catalog
        .add("first", component_bytes(wit, "provider"))
        .unwrap();
    let second = catalog
        .add("second", component_bytes(wit, "provider"))
        .unwrap();
    let policy = Policy::builder(&catalog)
        .link(caller, first)
        .unwrap()
        .link(caller, second)
        .unwrap()
        .build();
    assert!(matches!(
        Plan::new(catalog, policy),
        Err(PlanError::AmbiguousProvider { .. })
    ));
}

#[test]
fn plan_compares_complete_structural_types() {
    let caller_wit = structural_wit("s32", "safe", "polite", "choice", "caller", "import");
    let provider_wit = structural_wit("u32", "careful", "quiet", "selection", "provider", "export");
    let mut catalog = Catalog::new().unwrap();
    let caller = catalog
        .add("caller", component_bytes(&caller_wit, "caller"))
        .unwrap();
    let provider = catalog
        .add("provider", component_bytes(&provider_wit, "provider"))
        .unwrap();
    let policy = Policy::builder(&catalog)
        .link(caller, provider)
        .unwrap()
        .build();
    let error = match Plan::new(catalog, policy) {
        Ok(_) => panic!("mismatched structural types unexpectedly planned"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("type mismatch"));
    assert!(error.contains("variant"));
    assert!(error.contains("enum"));
    assert!(error.contains("flags"));
    assert!(error.contains("record"));
}

#[test]
fn registry_import_requires_explicit_policy_authority() {
    let mut catalog = Catalog::new().unwrap();
    let dynamic = catalog
        .add(
            "dynamic",
            component_bytes(include_str!("../wit/core.wit"), "consumer"),
        )
        .unwrap();
    let policy = Policy::builder(&catalog).include(dynamic).unwrap().build();
    assert!(matches!(
        Plan::new(catalog, policy),
        Err(PlanError::RegistryNotEnabled { .. })
    ));
}

#[test]
fn unused_registry_authority_is_harmless() {
    let mut catalog = Catalog::new().unwrap();
    let component = catalog
        .add(
            "plain",
            component_bytes(include_str!("../wit/core.wit"), "plugin"),
        )
        .unwrap();
    let policy = Policy::builder(&catalog)
        .enable_registry(component)
        .unwrap()
        .build();
    let plan = Plan::new(catalog, policy).unwrap();
    assert!(Runtime::new(plan).is_ok());
}

#[test]
fn plan_rejects_ambiguous_and_unencodable_dynamic_grants() {
    let provider_wit = r#"package demo:dynamic-types@0.1.0;
interface api { run: func(value: s64) -> s64; }
world provider { export api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let dynamic = catalog
        .add(
            "dynamic",
            component_bytes(include_str!("../wit/core.wit"), "consumer"),
        )
        .unwrap();
    let provider = catalog
        .add("provider", component_bytes(provider_wit, "provider"))
        .unwrap();
    let policy = Policy::builder(&catalog)
        .allow_lookup(dynamic, provider, "demo:dynamic-types/api@0.1.0#run")
        .unwrap()
        .build();
    assert!(matches!(
        Plan::new(catalog, policy),
        Err(PlanError::UnsupportedDynamicType { .. })
    ));

    let provider_wit = r#"package demo:dynamic-duplicate@0.1.0;
interface api { run: func(value: string) -> string; }
world provider { export api; }"#;
    let mut catalog = Catalog::new().unwrap();
    let dynamic = catalog
        .add(
            "dynamic",
            component_bytes(include_str!("../wit/core.wit"), "consumer"),
        )
        .unwrap();
    let first = catalog
        .add("first", component_bytes(provider_wit, "provider"))
        .unwrap();
    let second = catalog
        .add("second", component_bytes(provider_wit, "provider"))
        .unwrap();
    let policy = Policy::builder(&catalog)
        .allow_lookup(dynamic, first, "demo:dynamic-duplicate/api@0.1.0#run")
        .unwrap()
        .allow_lookup(dynamic, second, "demo:dynamic-duplicate/api@0.1.0#run")
        .unwrap()
        .build();
    assert!(matches!(
        Plan::new(catalog, policy),
        Err(PlanError::AmbiguousLookup { .. })
    ));
}

#[test]
fn fuel_trap_marks_only_the_looping_component_unhealthy() {
    let bytes = wat::parse_str(
        r#"(component
            (core module $m (func (export "loop") (loop $again (br $again))))
            (core instance $i (instantiate $m))
            (func $loop (canon lift (core func $i "loop")))
            (instance $api (export "run" (func $loop)))
            (export "demo:fuel/api@0.1.0" (instance $api)))"#,
    )
    .unwrap();
    let mut catalog = Catalog::new().unwrap();
    let looping = catalog.add("looping", bytes).unwrap();
    let policy = Policy::builder(&catalog).include(looping).unwrap().build();
    let runtime = Runtime::new(Plan::new(catalog, policy).unwrap()).unwrap();
    assert!(matches!(
        runtime.call(looping, "demo:fuel/api@0.1.0#run", &[]),
        Err(RuntimeError::Trapped { .. })
    ));
    assert!(!runtime.is_healthy(looping).unwrap());
    assert!(matches!(
        runtime.call(looping, "demo:fuel/api@0.1.0#run", &[]),
        Err(RuntimeError::Unhealthy { .. })
    ));
}

fn structural_wit(
    payload: &str,
    enum_case: &str,
    flag: &str,
    record_field: &str,
    world: &str,
    direction: &str,
) -> String {
    format!(
        r#"package demo:structural@0.1.0;

interface api {{
  variant choice {{ text(string), number({payload}) }}
  enum mode {{ fast, {enum_case} }}
  flags options {{ loud, {flag} }}
  record request {{ {record_field}: choice, mode: mode, options: options }}
  run: func(input: request) -> request;
}}

world {world} {{ {direction} api; }}"#
    )
}
