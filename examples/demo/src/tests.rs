//! Exercises security properties that would distract from the narrated demo.

use super::{HOST_SERVICES, artifacts, bindings};
use anyhow::Result;
use lockgate::{Application, Policy};

#[test]
fn filereader_cannot_read_outside_its_preopened_directory() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new()?;
    let filereader = app.add::<bindings::FileReaderPlugin>(
        "filereader",
        artifacts::component_bytes("filereader")?,
    )?;
    let policy = Policy::builder(app.catalog())
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = app.runtime(policy).build()?;

    let value = runtime.with_component(filereader, |store, bindings| {
        Ok(bindings
            .demo_host_file_reader()
            .call_read(&mut *store, "/etc/passwd")?)
    })?;

    assert!(matches!(value, Err(message) if !message.is_empty()));
    assert!(runtime.is_healthy(filereader)?);
    Ok(())
}
