//! Framework-owned configuration bindings and context-free schema probing.

use std::sync::Arc;

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{ResourceLimiter, Store};

use super::exec::{ExecEngine, ExecLimits, StoreCtx};

const MAX_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;

wasmtime::component::bindgen!({
    path: "wit",
    world: "plugin",
    imports: { default: async },
    exports: { default: async },
});

pub(crate) const SCHEMA_INTERFACE: &str = "lockgate:config/schema";

struct SchemaStore {
    limiter: SchemaMemoryLimiter,
}

impl lockgate::config::settings::Host for SchemaStore {
    async fn get_json(&mut self) -> Result<String, lockgate::config::settings::GetError> {
        Err(lockgate::config::settings::GetError::NotReady)
    }
}

impl ExecEngine {
    /// Runs the one pre-admission guest call without application data or imports.
    pub(crate) async fn fetch_settings_schema(
        &self,
        component: &Component,
        limits: ExecLimits,
    ) -> wasmtime::Result<Option<String>> {
        if component.get_export_index(None, SCHEMA_INTERFACE).is_none() {
            return Ok(None);
        }

        let mut linker = Linker::new(self.engine());
        Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |store| store)?;
        // Application imports are deliberately replaced by trap stubs. They
        // can satisfy typechecking, but can never reach application code.
        linker.define_unknown_imports_as_traps(component)?;

        let mut store = Store::new(
            self.engine(),
            SchemaStore {
                limiter: SchemaMemoryLimiter {
                    max_memory_bytes: limits.max_memory_bytes,
                },
            },
        );
        store.limiter(|store| &mut store.limiter);
        store.set_epoch_deadline(u64::MAX);
        store.set_fuel(limits.instantiation_fuel)?;

        let bindings = Plugin::instantiate_async(&mut store, component, &linker).await?;
        let schema = bindings
            .lockgate_config_schema()
            .call_settings_schema(&mut store)
            .await?;
        Ok(Some(schema))
    }
}

struct SchemaMemoryLimiter {
    max_memory_bytes: usize,
}

pub(crate) struct ValidatedSettings {
    json: String,
}

impl ValidatedSettings {
    pub(crate) fn into_state(self) -> SettingsState {
        SettingsState::Ready(self.json.into())
    }
}

#[derive(Clone)]
pub(crate) enum SettingsState {
    NotReady,
    Ready(Arc<str>),
}

impl<S: Send + Sync + 'static> lockgate::config::settings::Host for StoreCtx<S> {
    async fn get_json(&mut self) -> Result<String, lockgate::config::settings::GetError> {
        match self.settings() {
            SettingsState::NotReady => Err(lockgate::config::settings::GetError::NotReady),
            SettingsState::Ready(json) => Ok(json.to_string()),
        }
    }
}

pub(crate) fn add_settings_to_linker<S>(linker: &mut Linker<StoreCtx<S>>) -> wasmtime::Result<()>
where
    S: Send + Sync + 'static,
{
    lockgate::config::settings::add_to_linker::<_, HasSelf<_>>(linker, |store| store)
}

#[derive(Debug)]
pub(crate) enum SettingsValidationError {
    SettingsWithoutSchema,
    SchemaTooLarge { actual: usize, maximum: usize },
    SchemaTooDeep { maximum: usize },
    SchemaMalformed { message: String },
    NonLocalReference { reference: String },
    InvalidSchema { message: String },
    InvalidSettings { message: String },
}

pub(crate) fn validate_settings(
    schema: Option<&str>,
    settings: Option<serde_json::Value>,
) -> Result<ValidatedSettings, SettingsValidationError> {
    let Some(schema) = schema else {
        return match settings {
            Some(_) => Err(SettingsValidationError::SettingsWithoutSchema),
            None => Ok(ValidatedSettings {
                json: "{}".to_owned(),
            }),
        };
    };
    if schema.len() > MAX_SCHEMA_BYTES {
        return Err(SettingsValidationError::SchemaTooLarge {
            actual: schema.len(),
            maximum: MAX_SCHEMA_BYTES,
        });
    }

    let schema: serde_json::Value =
        serde_json::from_str(schema).map_err(|error| SettingsValidationError::SchemaMalformed {
            message: error.to_string(),
        })?;
    if json_depth(&schema) > MAX_SCHEMA_DEPTH {
        return Err(SettingsValidationError::SchemaTooDeep {
            maximum: MAX_SCHEMA_DEPTH,
        });
    }
    if let Some(reference) = non_local_reference(&schema) {
        return Err(SettingsValidationError::NonLocalReference {
            reference: reference.to_owned(),
        });
    }

    // No resolver features are enabled for `jsonschema`, so validator
    // construction cannot perform filesystem or network retrieval.
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| SettingsValidationError::InvalidSchema {
            message: error.to_string(),
        })?;
    let settings = settings.unwrap_or_else(|| serde_json::json!({}));
    if let Err(error) = validator.validate(&settings) {
        return Err(SettingsValidationError::InvalidSettings {
            message: error.to_string(),
        });
    }
    Ok(ValidatedSettings {
        json: serde_json::to_string(&settings)
            .expect("serializing a serde_json::Value cannot fail"),
    })
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn non_local_reference(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::Array(values) => values.iter().find_map(non_local_reference),
        serde_json::Value::Object(values) => {
            for keyword in ["$ref", "$dynamicRef"] {
                if let Some(reference) = values.get(keyword).and_then(serde_json::Value::as_str)
                    && !reference.starts_with('#')
                {
                    return Some(reference);
                }
            }
            values.values().find_map(non_local_reference)
        }
        _ => None,
    }
}

impl ResourceLimiter for SchemaMemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_memory_bytes)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::{
        MAX_SCHEMA_BYTES, MAX_SCHEMA_DEPTH, SettingsValidationError, validate_settings,
    };
    use super::super::exec::{ExecEngine, ExecLimits};
    use wit_parser::Resolve;

    #[test]
    fn owned_config_package_has_the_framework_contract() {
        assert_eq!(
            include_str!("../wit/config.wit"),
            include_str!("../tests/fixtures/config-guest/wit/deps/lockgate-config/config.wit"),
            "the raw guest fixture must vendor the owned config contract exactly"
        );
        let mut resolve = Resolve::new();
        let (package, _) = resolve
            .push_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/wit"))
            .unwrap();
        let world = resolve.select_world(&[package], Some("plugin")).unwrap();
        let world = &resolve.worlds[world];

        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.exports.len(), 1);
        assert!(
            world
                .imports
                .keys()
                .any(|name| resolve.name_world_key(name) == "lockgate:config/settings")
        );
        assert!(
            world
                .exports
                .keys()
                .any(|name| resolve.name_world_key(name) == "lockgate:config/schema")
        );
    }

    #[tokio::test]
    async fn schema_probe_has_no_application_import_implementation() {
        let engine = ExecEngine::new().unwrap();
        let component = engine.compile(
            &wat::parse_str(
                r#"(component
                    (type $application (instance
                        (export "touch" (func))
                    ))
                    (import "test:application/host" (instance $application-instance (type $application)))

                    (core module $guest
                        (memory (export "memory") 1)
                        (data (i32.const 64) "{\22type\22:\22object\22}")
                        (func (export "settings-schema") (result i32)
                            (i32.store (i32.const 8) (i32.const 64))
                            (i32.store offset=4 (i32.const 8) (i32.const 17))
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
            )
            .unwrap(),
        )
        .unwrap();

        let schema = engine
            .fetch_settings_schema(
                &component,
                ExecLimits {
                    instantiation_fuel: 1_000_000,
                    max_memory_bytes: 1024 * 1024,
                },
            )
            .await
            .unwrap();
        assert_eq!(schema.as_deref(), Some(r#"{"type":"object"}"#));

        let error = engine
            .fetch_settings_schema(
                &component,
                ExecLimits {
                    instantiation_fuel: 1_000_000,
                    max_memory_bytes: 0,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("memory"));
    }

    #[test]
    fn schema_references_are_local_only() {
        let error = validate_settings(
            Some(r#"{"$ref":"https://example.invalid/settings.json"}"#),
            None,
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            SettingsValidationError::NonLocalReference { reference }
                if reference == "https://example.invalid/settings.json"
        ));

        validate_settings(
            Some(r##"{"$defs":{"value":{"type":"string"}},"$ref":"#/$defs/value"}"##),
            Some(serde_json::json!("local")),
        )
        .unwrap();
    }

    #[test]
    fn invalid_json_schema_reports_the_offending_value() {
        let error = validate_settings(Some(r#"{"type":"not-a-real-type"}"#), None)
            .err()
            .unwrap();
        assert!(matches!(
            error,
            SettingsValidationError::InvalidSchema { ref message }
                if message.contains("not-a-real-type")
        ));
    }

    #[test]
    fn schema_size_and_depth_have_explicit_limits() {
        let error = validate_settings(Some(&" ".repeat(MAX_SCHEMA_BYTES + 1)), None)
            .err()
            .unwrap();
        assert!(matches!(
            error,
            SettingsValidationError::SchemaTooLarge { actual, maximum }
                if actual == MAX_SCHEMA_BYTES + 1 && maximum == MAX_SCHEMA_BYTES
        ));

        let schema = format!(
            "{}true{}",
            "[".repeat(MAX_SCHEMA_DEPTH + 1),
            "]".repeat(MAX_SCHEMA_DEPTH + 1)
        );
        let error = validate_settings(Some(&schema), None).err().unwrap();
        assert!(matches!(
            error,
            SettingsValidationError::SchemaTooDeep { maximum }
                if maximum == MAX_SCHEMA_DEPTH
        ));
    }
}
