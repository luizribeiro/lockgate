//! Exercises security properties that would distract from the narrated demo.

use super::{HOST_SERVICES, artifacts, bindings};
use anyhow::Result;
use lockgate::Application;

#[tokio::test]
async fn filereader_cannot_read_outside_its_preopened_directory() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new(())?;
    let filereader =
        app.add::<bindings::FileReaderPlugin>(artifacts::component_bytes("filereader")?)?;
    let runtime = app
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .run()
        .await?;

    let value = runtime
        .component(filereader)
        .read("/etc/passwd".to_owned())
        .await?;

    assert!(matches!(value, Err(message) if !message.is_empty()));
    Ok(())
}

#[tokio::test]
async fn one_instance_can_be_called_through_multiple_roles() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new(())?;
    let (reader, runnable) = app.add::<(bindings::FileReaderPlugin, bindings::RunnablePlugin)>(
        artifacts::component_bytes("filereader")?,
    )?;
    let runtime = app
        .allow_host_import(reader, HOST_SERVICES)?
        .read_only_dir(reader, root.join("sandbox/shared"), "/shared")?
        .run()
        .await?;

    let run = runtime.component(runnable).run().await?;
    let read = runtime
        .component(reader)
        .read("/shared/allowed.txt".to_owned())
        .await?;

    assert!(matches!(run, Ok(message) if !message.is_empty()));
    assert!(matches!(read, Ok(message) if !message.is_empty()));
    Ok(())
}
