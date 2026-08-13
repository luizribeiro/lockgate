use super::*;
use wit_component::{
    ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata, encode,
};
use wit_parser::{ManglingAndAbi, Resolve};

#[path = "../../tests/common/mod.rs"]
mod fixtures;

fn component(wit: &str) -> Vec<u8> {
    component_world(wit, None)
}

fn component_world(wit: &str, world: Option<&str>) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.select_world(&[package], world).unwrap();
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

fn compound_component(world: &str) -> Vec<u8> {
    component_world(
        include_str!("../../tests/data/export_validation/compound_wrappers.wit"),
        Some(world),
    )
}

fn component_wat(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

#[test]
fn accepts_value_only_component_wit() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/value_only.wit"
    ));

    validate_value_only_exports(&bytes).unwrap();
}

#[test]
fn accepts_existing_exec_fixture() {
    validate_value_only_exports(fixtures::exec_fixture()).unwrap();
}

#[test]
fn reports_component_wit_decode_failures() {
    let error = validate_value_only_exports(b"not a component").unwrap_err();

    assert!(matches!(error, ValidationError::Decode { .. }));
    assert!(
        error
            .to_string()
            .starts_with("failed to decode component WIT:")
    );
}

#[test]
fn rejects_encoded_wit_packages() {
    let mut resolve = Resolve::new();
    let package = resolve
        .push_str(
            "package.wit",
            "package test:types; interface api { run: func(); }",
        )
        .unwrap();
    let bytes = encode(&resolve, package).unwrap();

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err(),
        ValidationError::NotComponent
    );
}

#[test]
fn rejects_world_level_exported_functions_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/world_function.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `root#run`: offending type `world-level exported function` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_world_level_exported_types_with_guidance() {
    let bytes = component_wat(include_str!(
        "../../tests/data/export_validation/world_type.wat"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `root#<type payload>`: offending type `world-level non-interface export` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_unused_exported_resource_declarations_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/unused_resource.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:unused-resource/api#<type file>`: offending type `resource file` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_unused_use_aliased_resource_declarations() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/unused_resource_alias.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:alias-unused/api@0.1.0#<type file>`: offending type `resource file` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_unused_multi_hop_resource_aliases() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/unused_resource_multi_alias.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:alias-multi/api@0.1.0#<type document>`: offending type `resource file` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_nested_owned_handles_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/owned_handle.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:owned-handle/api#run`: offending type `own<file>` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_nested_borrowed_handles_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/borrowed_handle.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:borrowed-handle/api#run`: offending type `borrow<file>` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_nested_futures_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/future.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:future-export/api#run`: offending type `future` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_nested_streams_with_guidance() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/stream.wit"
    ));

    assert_eq!(
        validate_value_only_exports(&bytes).unwrap_err().to_string(),
        "unsupported export `test:stream-export/api#run`: offending type `stream` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn reports_nested_error_context_before_component_compilation() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/error_context.wit"
    ));
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model(true)
        .wasm_component_model_async(true);
    let engine = wasmtime::Engine::new(&config).unwrap();

    let compiler_error = wasmtime::component::Component::new(&engine, &bytes).unwrap_err();
    assert!(
        format!("{compiler_error:#}")
            .contains("requires the component model error-context feature"),
        "unexpected Wasmtime error: {compiler_error:#}"
    );

    let error = validate_value_only_exports(&bytes).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:error-context-export/api#run`: offending type `error-context` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_streams_nested_in_records() {
    let error = validate_value_only_exports(&compound_component("record-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/record-api#run`: offending type `stream` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_owned_handles_nested_in_variants() {
    let error = validate_value_only_exports(&compound_component("variant-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/variant-api#run`: offending type `own<file>` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_futures_nested_in_lists() {
    let error = validate_value_only_exports(&compound_component("list-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/list-api#run`: offending type `future` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_futures_nested_in_fixed_length_lists() {
    let error = validate_value_only_exports(&compound_component("fixed-list-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/fixed-list-api#run`: offending type `future` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_error_contexts_nested_in_tuples() {
    let error = validate_value_only_exports(&compound_component("tuple-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/tuple-api#run`: offending type `error-context` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_streams_nested_in_results() {
    let error = validate_value_only_exports(&compound_component("result-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/result-api#run`: offending type `stream` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}

#[test]
fn rejects_futures_nested_in_maps() {
    let error = validate_value_only_exports(&compound_component("map-fixture")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported export `test:compound-wrappers/map-api#run`: offending type `future` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
}
