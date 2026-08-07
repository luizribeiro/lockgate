//! Builds and narrates typed host calls, sibling forwarding, and filesystem isolation.

use anyhow::Result;
use lockgate::{Catalog, ComponentId, Event, HasHost, PluginStore, Policy, Runtime};
use wasmtime::component::Linker;

mod runnable_bindings {
    wasmtime::component::bindgen!({
        path: "wit/packages/host",
        world: "runnable-plugin",
    });
}

mod file_reader_bindings {
    wasmtime::component::bindgen!({
        path: "wit/packages/host",
        world: "file-reader-plugin",
    });
}

mod artifacts;
#[cfg(test)]
mod tests;

const HOST_SERVICES: &str = "demo:host/services@0.1.0";

struct DemoHost {
    component: String,
}

impl runnable_bindings::demo::host::services::Host for DemoHost {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.component);
    }
}

impl file_reader_bindings::demo::host::services::Host for DemoHost {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.component);
    }
}

fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut catalog = Catalog::new()?;
    let greeter = catalog.add("greeter", artifacts::component_bytes("greeter")?)?;
    let caller = catalog.add("caller", artifacts::component_bytes("caller")?)?;
    let filereader = catalog.add("filereader", artifacts::component_bytes("filereader")?)?;
    print_catalog(&catalog);

    let policy = Policy::builder(&catalog)
        .link(caller, greeter)?
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = Runtime::builder(catalog, policy)
        .with_host(
            |_, name| DemoHost {
                component: name.into(),
            },
            move |component, linker| configure_host(component, caller, filereader, linker),
        )
        .require_world(caller, |linker, component| {
            let pre = linker.instantiate_pre(component)?;
            runnable_bindings::RunnablePluginPre::new(pre)?;
            Ok(())
        })?
        .require_world(filereader, |linker, component| {
            let pre = linker.instantiate_pre(component)?;
            file_reader_bindings::FileReaderPluginPre::new(pre)?;
            Ok(())
        })?
        .with_observer(print_event)
        .build()?;
    println!("\n[call] caller.run()");
    let value = runtime.with_instance(caller, |store, instance| {
        let bindings = runnable_bindings::RunnablePlugin::new(&mut *store, instance)?;
        Ok(bindings.demo_host_runnable().call_run(&mut *store)?)
    })?;
    println!("  => {value:?}");

    println!("\n[call] filereader.run()");
    let value = runtime.with_instance(filereader, |store, instance| {
        let bindings = file_reader_bindings::FileReaderPlugin::new(&mut *store, instance)?;
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
    linker: &mut Linker<PluginStore<DemoHost>>,
) -> anyhow::Result<()> {
    if component == caller {
        runnable_bindings::RunnablePlugin::add_to_linker::<_, HasHost<DemoHost>>(
            linker,
            PluginStore::host_mut,
        )?;
    } else if component == filereader {
        file_reader_bindings::FileReaderPlugin::add_to_linker::<_, HasHost<DemoHost>>(
            linker,
            PluginStore::host_mut,
        )?;
    }
    Ok(())
}

fn print_event(event: Event) {
    match event {
        Event::DirectCall { target } => println!("  [direct] -> {target}"),
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
