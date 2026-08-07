//! Exercises security properties that would distract from the narrated demo.

use super::{DemoHost, HOST_SERVICES, artifacts, file_reader_bindings};
use anyhow::Result;
use lockgate::{Catalog, Policy, Runtime};
use wasmtime::component::HasSelf;

#[test]
fn filereader_cannot_read_outside_its_preopened_directory() -> Result<()> {
    let root = artifacts::root()?;
    let mut catalog = Catalog::new()?;
    let filereader = catalog.add("filereader", artifacts::component_bytes("filereader")?)?;
    let policy = Policy::builder(&catalog)
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = Runtime::builder(catalog, policy)
        .with_host(
            |_, name| DemoHost {
                component: name.into(),
            },
            |_, linker| {
                file_reader_bindings::FileReaderPlugin::add_to_linker::<_, HasSelf<_>>(
                    linker,
                    |state| state,
                )?;
                Ok(())
            },
        )
        .require_world(filereader, |linker, component| {
            let pre = linker.instantiate_pre(component)?;
            file_reader_bindings::FileReaderPluginPre::new(pre)?;
            Ok(())
        })?
        .build()?;

    let value = runtime.with_instance(filereader, |store, instance| {
        let bindings = file_reader_bindings::FileReaderPlugin::new(&mut *store, instance)?;
        Ok(bindings
            .demo_host_file_reader()
            .call_read(&mut *store, "/etc/passwd")?)
    })?;

    assert!(matches!(value, Err(message) if !message.is_empty()));
    assert!(runtime.is_healthy(filereader)?);
    Ok(())
}
