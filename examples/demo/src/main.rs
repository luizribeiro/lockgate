//! Builds and narrates direct, filesystem, and dynamic Lockgate calls.

use anyhow::Result;
use lockgate::{Catalog, ComponentId, Event, PluginStore, Policy, Runtime, Val};
use wasmtime::component::{HasSelf, Linker};

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

impl runnable_bindings::demo::host::services::Host for PluginStore<DemoHost> {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.host().component);
    }
}

impl file_reader_bindings::demo::host::services::Host for PluginStore<DemoHost> {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.host().component);
    }
}

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
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .allow_lookup(dynamic, greeter, "demo:greeter/greeter@0.1.0#greet")?
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

    println!("\n[call] dynamic.run()");
    let values = runtime.call(dynamic, "demo:dynamic/runner@0.1.0#run", &[])?;
    println!("  => {}", render_values(&values));

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
        runnable_bindings::RunnablePlugin::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
    } else if component == filereader {
        file_reader_bindings::FileReaderPlugin::add_to_linker::<_, HasSelf<_>>(linker, |state| {
            state
        })?;
    }
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
