//! Builds and narrates typed host calls, sibling forwarding, and filesystem isolation.

use anyhow::Result;
use lockgate::{Application, HostContext};

mod bindings {
    lockgate::bindings! {
        path: "wit/deps/demo-host",
        worlds: {
            RunnablePlugin: "runnable-plugin",
            FileReaderPlugin: "file-reader-plugin",
        },
    }
}

mod greeter_bindings {
    lockgate::bindings! {
        path: "wit/deps/demo-greeter",
        worlds: {
            GreeterPlugin: "greeter-plugin",
        },
    }
}

mod artifacts;
#[cfg(test)]
mod tests;

const HOST_SERVICES: &str = "demo:host/services@0.1.0";

impl bindings::demo::host::services::Host for HostContext<()> {}

impl bindings::demo::host::services::HostWithStore<lockgate::__private::PluginStore<()>>
    for HostContext<()>
{
    async fn log(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<()>, Self>,
        message: String,
    ) {
        accessor.with(|mut access| {
            println!("  [host] {}: {message}", access.get().plugin().id());
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new(())?;
    let greeter =
        app.add::<greeter_bindings::GreeterPlugin>(artifacts::component_bytes("greeter")?)?;
    let caller = app.add::<bindings::RunnablePlugin>(artifacts::component_bytes("caller")?)?;
    let (filereader, filereader_runnable) = app
        .add::<(bindings::FileReaderPlugin, bindings::RunnablePlugin)>(
            artifacts::component_bytes("filereader")?,
        )?;
    let runtime = app
        .link(caller, greeter)?
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .run()
        .await?;
    println!("\n[call] caller.run()");
    let value = runtime.component(caller).run().await?;
    println!("  => {value:?}");

    println!("\n[call] filereader.run()");
    let value = runtime.component(filereader_runnable).run().await?;
    println!("  => {value:?}");

    println!("\n[host] still running");
    Ok(())
}
