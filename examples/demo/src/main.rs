//! Builds and narrates typed host calls, sibling forwarding, and filesystem isolation.

use anyhow::Result;
use lockgate::{Catalog, ComponentId, Event, HasHost, HostContext, PluginStore, Policy, Runtime};
use wasmtime::component::Linker;

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
    let mut catalog = Catalog::new()?;
    let greeter = catalog.add_untyped("greeter", artifacts::component_bytes("greeter")?)?;
    let caller =
        catalog.add::<bindings::RunnablePlugin>("caller", artifacts::component_bytes("caller")?)?;
    let filereader = catalog.add::<bindings::FileReaderPlugin>(
        "filereader",
        artifacts::component_bytes("filereader")?,
    )?;
    print_catalog(&catalog);

    let policy = Policy::builder(&catalog)
        .link(caller, greeter)?
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = Runtime::builder(catalog, policy)
        .with_host(
            |_, _| (),
            move |component, linker| {
                configure_host(component, caller.id(), filereader.id(), linker)
            },
        )
        .with_observer(print_event)
        .build()?;
    println!("\n[call] caller.run()");
    let value = runtime.with_component(caller, |store, bindings| {
        Ok(bindings.demo_host_runnable().call_run(&mut *store)?)
    })?;
    println!("  => {value:?}");

    println!("\n[call] filereader.run()");
    let value = runtime.with_component(filereader, |store, bindings| {
        Ok(bindings.demo_host_runnable().call_run(&mut *store)?)
    })?;
    println!("  => {value:?}");

    println!("\n[host] still running");
    Ok(())
}

fn configure_host(
    component: ComponentId,
    caller: ComponentId,
    filereader: ComponentId,
    linker: &mut Linker<PluginStore<()>>,
) -> anyhow::Result<()> {
    if component == caller || component == filereader {
        bindings::demo::host::services::add_to_linker::<_, HasHost<()>>(
            linker,
            PluginStore::context_mut,
        )?;
    }
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
