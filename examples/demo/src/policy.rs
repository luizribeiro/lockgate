//! Loads the five demo artifacts and expresses all runtime authority through Lockgate handles.

use anyhow::{Context, Result};
use lockgate::{Catalog, ComponentId, ExportId, Plan, Policy};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct DemoPlan {
    pub(crate) plan: Plan,
    pub(crate) caller_run: ExportId,
    pub(crate) filereader_run: ExportId,
    pub(crate) dynamic_run: ExportId,
}

pub(crate) fn build() -> Result<DemoPlan> {
    let root = root()?;
    let mut catalog = Catalog::new()?;
    let greeter = add(&mut catalog, &root, "greeter")?;
    let caller = add(&mut catalog, &root, "caller")?;
    let filereader = add(&mut catalog, &root, "filereader")?;
    let naughty = add(&mut catalog, &root, "naughty")?;
    let dynamic = add(&mut catalog, &root, "dynamic")?;
    print_catalog(&catalog, [greeter, caller, filereader, naughty, dynamic])?;

    let greet = catalog.export(greeter, "demo:greeter/greeter@0.1.0#greet")?;
    let caller_run = catalog.export(caller, "demo:caller/runner@0.1.0#run")?;
    let filereader_run = catalog.export(filereader, "demo:filereader/runner@0.1.0#run")?;
    let dynamic_run = catalog.export(dynamic, "demo:dynamic/runner@0.1.0#run")?;
    let policy = Policy::builder(&catalog)
        .link(caller, greeter)?
        .allow_lookup(dynamic, greet)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();

    Ok(DemoPlan {
        plan: Plan::new(catalog, policy)?,
        caller_run,
        filereader_run,
        dynamic_run,
    })
}

pub(crate) fn naughty_error() -> Result<String> {
    let root = root()?;
    let mut catalog = Catalog::new()?;
    let greeter = add(&mut catalog, &root, "greeter")?;
    let naughty = add(&mut catalog, &root, "naughty")?;
    let policy = Policy::builder(&catalog)
        .include(greeter)?
        .include(naughty)?
        .build();
    match Plan::new(catalog, policy) {
        Ok(_) => anyhow::bail!("naughty unexpectedly produced a valid plan"),
        Err(error) => Ok(error.to_string()),
    }
}

fn root() -> Result<PathBuf> {
    Path::new(env!("LOCKGATE_DEMO_ROOT"))
        .canonicalize()
        .context("failed to resolve the staged demo root")
}

fn add(catalog: &mut Catalog, root: &Path, name: &str) -> Result<ComponentId> {
    let path = root
        .join("components")
        .join(name)
        .join(format!("{name}.wasm"));
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(catalog.add(name, bytes)?)
}

fn print_catalog(catalog: &Catalog, components: [ComponentId; 5]) -> Result<()> {
    for component in components {
        let info = catalog.component(component)?;
        let exports = info
            .exports()
            .iter()
            .map(|export| {
                let signature = export.signature();
                let results = match signature.results() {
                    [] => "()".into(),
                    [result] => result.clone(),
                    results => format!("({})", results.join(", ")),
                };
                format!(
                    "{}({}) -> {results}",
                    export.target(),
                    signature.params().join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!("[load] {:<12} ok   provides {exports}", info.name());
    }
    Ok(())
}
