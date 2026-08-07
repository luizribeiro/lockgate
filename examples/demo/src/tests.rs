//! Exercises security properties that would distract from the narrated demo.

use super::artifacts;
use anyhow::Result;
use lockgate::{Catalog, Plan, Policy, Runtime, Val};

#[test]
fn filereader_cannot_read_outside_its_preopened_directory() -> Result<()> {
    let root = artifacts::root()?;
    let mut catalog = Catalog::new()?;
    let filereader = catalog.add("filereader", artifacts::component_bytes("filereader")?)?;
    let policy = Policy::builder(&catalog)
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = Runtime::new(Plan::new(catalog, policy)?)?;

    let values = runtime.call(
        filereader,
        "demo:filereader/runner@0.1.0#read",
        &[Val::String("/etc/passwd".into())],
    )?;

    assert!(matches!(
        values.as_slice(),
        [Val::Result(Err(Some(error)))]
            if matches!(error.as_ref(), Val::String(message) if !message.is_empty())
    ));
    assert!(runtime.is_healthy(filereader)?);
    Ok(())
}
