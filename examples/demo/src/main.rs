//! Builds and narrates the end-to-end Lockgate demo, including direct,
//! filesystem, dynamic, and rejected calls.

use anyhow::Result;
use lockgate::{Catalog, ComponentId, Event, Plan, Policy, Runtime, Val};

mod artifacts;

fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut catalog = Catalog::new()?;
    let greeter = catalog.add("greeter", artifacts::component_bytes("greeter")?)?;
    let caller = catalog.add("caller", artifacts::component_bytes("caller")?)?;
    let filereader = catalog.add("filereader", artifacts::component_bytes("filereader")?)?;
    let naughty = catalog.add("naughty", artifacts::component_bytes("naughty")?)?;
    let dynamic = catalog.add("dynamic", artifacts::component_bytes("dynamic")?)?;
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
    let plan = Plan::new(catalog, policy)?;

    let mut naughty_catalog = Catalog::new()?;
    let greeter = naughty_catalog.add("greeter", artifacts::component_bytes("greeter")?)?;
    let naughty = naughty_catalog.add("naughty", artifacts::component_bytes("naughty")?)?;
    let naughty_policy = Policy::builder(&naughty_catalog)
        .include(greeter)?
        .include(naughty)?
        .build();
    let Err(naughty_error) = Plan::new(naughty_catalog, naughty_policy) else {
        anyhow::bail!("naughty unexpectedly produced a valid plan");
    };
    println!();
    println!("[plan] naughty  REFUSED {naughty_error}");

    let runtime = Runtime::with_observer(plan, |event| match event {
        Event::DirectCall { target } => println!("  [direct] -> {target}"),
        Event::DynamicLookup {
            caller,
            target,
            allowed,
        } => println!(
            "  [dynamic] {caller} -> {target}  {}",
            if allowed { "ALLOWED" } else { "DENIED" }
        ),
    })?;
    println!("\n[call] caller.run()");
    let values = runtime.call(caller_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] filereader.run()");
    let values = runtime.call(filereader_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] naughty.run()\n  => unavailable (incomplete policy was refused)");

    println!("\n[call] dynamic.run()");
    let values = runtime.call(dynamic_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[host] still running");
    Ok(())
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

fn render_values(values: &[Val]) -> String {
    match values {
        [Val::Result(Ok(Some(value)))] => match value.as_ref() {
            Val::String(value) => format!("{value:?}"),
            other => format!("{other:?}"),
        },
        [Val::List(values)] => format!("{values:?}"),
        other => format!("{other:?}"),
    }
}
