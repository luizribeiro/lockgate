//! Builds and narrates direct, filesystem, and dynamic Lockgate calls.

use anyhow::Result;
use lockgate::{Catalog, Event, Policy, Runtime, Val};

mod artifacts;
#[cfg(test)]
mod tests;

fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut catalog = Catalog::new()?;
    let greeter = catalog.add("greeter", artifacts::component_bytes("greeter")?)?;
    let caller = catalog.add("caller", artifacts::component_bytes("caller")?)?;
    let filereader = catalog.add("filereader", artifacts::component_bytes("filereader")?)?;
    let dynamic = catalog.add("dynamic", artifacts::component_bytes("dynamic")?)?;
    print_catalog(&catalog);

    let policy = Policy::builder(&catalog)
        .link(caller, greeter)?
        .allow_lookup(dynamic, greeter, "demo:greeter/greeter@0.1.0#greet")?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = Runtime::builder(catalog, policy)
        .with_observer(print_event)
        .build()?;
    println!("\n[call] caller.run()");
    let values = runtime.call(caller, "demo:caller/runner@0.1.0#run", &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] filereader.run()");
    let values = runtime.call(filereader, "demo:filereader/runner@0.1.0#run", &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] dynamic.run()");
    let values = runtime.call(dynamic, "demo:dynamic/runner@0.1.0#run", &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[host] still running");
    Ok(())
}

fn print_event(event: Event) {
    match event {
        Event::DirectCall { target } => println!("  [direct] -> {target}"),
        Event::DynamicLookup {
            caller,
            target,
            allowed,
        } => println!(
            "  [dynamic] {caller} -> {target}  {}",
            if allowed { "ALLOWED" } else { "DENIED" }
        ),
    }
}

fn print_catalog(catalog: &Catalog) {
    for info in catalog.components() {
        let exports = info
            .exports()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("[load] {:<12} ok   provides {exports}", info.name());
    }
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
