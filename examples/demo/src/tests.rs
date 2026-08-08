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

    let value = runtime
        .component(filereader)
        .file_reader()
        .read("/etc/passwd")?;

    assert!(matches!(value, Err(message) if !message.is_empty()));
    assert!(runtime.is_healthy(filereader)?);
    Ok(())
}

#[test]
fn one_instance_can_be_called_through_multiple_admitted_roles() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new()?;
    let reader = app.add::<bindings::FileReaderPlugin>(
        "filereader",
        artifacts::component_bytes("filereader")?,
    )?;
    let runnable = app.admit::<bindings::RunnablePlugin>(reader)?;
    let policy = Policy::builder(app.catalog())
        .allow_host_import(reader, HOST_SERVICES)?
        .read_only_dir(reader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = app.runtime(policy).build()?;

    let run = runtime.component(runnable).runnable().run()?;
    let read = runtime
        .component(reader)
        .file_reader()
        .read("/shared/allowed.txt")?;

    assert!(matches!(run, Ok(message) if !message.is_empty()));
    assert!(matches!(read, Ok(message) if !message.is_empty()));
    assert!(runtime.is_healthy(reader)?);
    Ok(())
}
