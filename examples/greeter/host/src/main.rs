use std::error::Error;
use std::path::{Path, PathBuf};

use lockgate::{HostBuilder, PluginConfig, RuntimeLimits};

const PLUGIN_ID: &str = "greeter";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/greeter/plugin/target/wasm32-wasip2/release/greeter_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/greeter/plugin/Cargo.toml --target wasm32-wasip2 --release";

lockgate::host_bindings!({
    path: "../wit",
    world: "plugin",
});

use greeter::HostExt;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PLUGIN_PATH));
    let bytes = read_component(&path)?;

    let mut builder = HostBuilder::new(())?;
    // prepare validates the declared identity, needs, and supported WIT shape.
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await?;
    // admit records consent, applies limits, and proves the component can start.
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await?;
    let host = builder.finish();

    let greeting = host.greeter(&plugin)?.greet("world").await?;
    println!("{greeting}");
    host.shutdown().await;
    Ok(())
}

fn read_component(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    std::fs::read(path).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "could not read plugin component at `{}`: {source}\n\
                 build the plugin first with:\n  {BUILD_COMMAND}",
                path.display()
            ),
        )
    })
}
