//! Framework-owned configuration bindings and context-free schema probing.

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{ResourceLimiter, Store};

use crate::exec::{ExecEngine, ExecLimits};

wasmtime::component::bindgen!({
    path: "wit",
    world: "plugin",
    imports: { default: async },
    exports: { default: async },
});

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the lifecycle consumes this probe in the next grain"
    )
)]
pub(crate) const SCHEMA_INTERFACE: &str = "lockgate:config/schema";

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the lifecycle consumes this probe in the next grain"
    )
)]
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
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the lifecycle consumes this probe in the next grain"
        )
    )]
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

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the lifecycle consumes this probe in the next grain"
    )
)]
struct SchemaMemoryLimiter {
    max_memory_bytes: usize,
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
    use crate::exec::{ExecEngine, ExecLimits};
    use wit_parser::Resolve;

    #[test]
    fn owned_config_package_has_the_framework_contract() {
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
    }
}
