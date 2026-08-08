//! Builds and narrates typed host calls, sibling forwarding, and filesystem isolation.

use anyhow::Result;
use lockgate::{Application, Catalog, Event, HostContext, Policy};

mod bindings {
    lockgate::bindings! {
        path: "wit/packages/host",
        worlds: {
            RunnablePlugin: "runnable-plugin",
            FileReaderPlugin: "file-reader-plugin",
        },
    }
}

mod artifacts;
#[cfg(test)]
mod tests;

const HOST_SERVICES: &str = "demo:host/services@0.1.0";

impl bindings::demo::host::services::Host for HostContext<()> {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.component().name());
    }
}

fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new()?;
    let greeter = app.add_untyped("greeter", artifacts::component_bytes("greeter")?)?;
    let caller =
        app.add::<bindings::RunnablePlugin>("caller", artifacts::component_bytes("caller")?)?;
    let filereader = app.add::<bindings::FileReaderPlugin>(
        "filereader",
        artifacts::component_bytes("filereader")?,
    )?;
    print_catalog(app.catalog());

    let policy = Policy::builder(app.catalog())
        .link(caller, greeter)?
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = app.runtime(policy).with_observer(print_event).build()?;
    println!("\n[call] caller.run()");
    let value = runtime.component(caller).runnable().run()?;
    println!("  => {value:?}");

    println!("\n[call] filereader.run()");
    let value = runtime.component(filereader).runnable().run()?;
    println!("  => {value:?}");

    println!("\n[host] still running");
    Ok(())
}

fn print_event(event: Event) {
    match event {
        Event::SiblingCall {
            caller,
            provider,
            target,
        } => println!("  [sibling] {caller} -> {provider} ({target})"),
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
