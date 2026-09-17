//! Exact `lockgate:plugin` payload bytes copied from the donor repository.

/// `lockgate-schema/src/lib.rs`'s stable-encoding test in the donor repository.
pub const SCHEMA_UNIT_TEST: &[u8] = br#"{"format":1,"id":"com.example.greeter","name":"Greeter","version":"1.2.3","description":"Returns greetings"}"#;

/// The section embedded in the donor repository's `fixtures/sync-export-component.wasm`.
pub const SYNC_EXPORT_COMPONENT: &[u8] = br#"{"format":1,"id":"sync-export","name":"Synchronous export fixture","version":"0.0.0","description":"Exercises synchronous host invocation"}"#;
