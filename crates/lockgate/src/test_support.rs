use wasm_encoder::{ComponentSection, CustomSection};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

pub(crate) fn component_from_wit(wit: &str) -> Vec<u8> {
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

pub(crate) fn settings_schema_component(schema: &str) -> Vec<u8> {
    let encoded = schema
        .as_bytes()
        .iter()
        .map(|byte| format!(r"\{byte:02x}"))
        .collect::<String>();
    wat::parse_str(format!(
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
    .unwrap()
}

pub(crate) fn with_section(mut component: Vec<u8>, name: &str, data: &[u8]) -> Vec<u8> {
    CustomSection {
        name: name.into(),
        data: data.into(),
    }
    .append_to_component(&mut component);
    component
}
