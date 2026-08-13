use super::*;
use wit_component::{
    ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata, encode,
};
use wit_parser::{ManglingAndAbi, Resolve};

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

#[test]
fn accepts_value_only_component_wit() {
    let bytes = component(include_str!(
        "../../tests/data/export_validation/value_only.wit"
    ));

    validate_value_only_exports(&bytes).unwrap();
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
