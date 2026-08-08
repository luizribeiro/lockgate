//! Builds and narrates typed host calls, sibling forwarding, and filesystem isolation.

use anyhow::Result;
use lockgate::{Application, HostContext};

mod bindings {
    lockgate::bindings! {
        path: "wit/packages/host",
        worlds: {
            RunnablePlugin: "runnable-plugin",
            FileReaderPlugin: "file-reader-plugin",
        },
    }
}

mod greeter_bindings {
    lockgate::bindings! {
        path: "wit/packages/greeter",
        worlds: {
            GreeterPlugin: "greeter-plugin",
        },
    }
}

mod artifacts;
#[cfg(test)]
mod tests;

const HOST_SERVICES: &str = "demo:host/services@0.1.0";

impl bindings::demo::host::services::Host for HostContext<()> {
    fn log(&mut self, message: String) {
        println!("  [host] {}: {message}", self.component_name());
    }
}

fn main() -> Result<()> {
    let root = artifacts::root()?;
    let mut app = Application::new()?;
    let greeter = app.add::<greeter_bindings::GreeterPlugin>(
        "greeter",
        artifacts::component_bytes("greeter")?,
    )?;
    let caller =
        app.add::<bindings::RunnablePlugin>("caller", artifacts::component_bytes("caller")?)?;
    let filereader = app.add::<bindings::FileReaderPlugin>(
        "filereader",
        artifacts::component_bytes("filereader")?,
    )?;
    let policy = app
        .policy()
        .link(caller, greeter)?
        .allow_host_import(caller, HOST_SERVICES)?
        .allow_host_import(filereader, HOST_SERVICES)?
        .read_only_dir(filereader, root.join("sandbox/shared"), "/shared")?
        .build();
    let runtime = app.runtime(policy)?;
    println!("\n[call] caller.run()");
    let value = runtime.component(caller).runnable().run()?;
    println!("  => {value:?}");

    println!("\n[call] filereader.run()");
    let value = runtime.component(filereader).runnable().run()?;
    println!("  => {value:?}");

    println!("\n[host] still running");
    Ok(())
}
