use std::error::Error;
use std::path::{Path, PathBuf};

use lockgate::{Acceptance, HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

const PLUGIN_ID: &str = "configuration";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/configuration/plugin/target/wasm32-wasip2/release/configuration_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/configuration/plugin/Cargo.toml --target wasm32-wasip2 --release";

lockgate::host_bindings!({
    path: "../wit",
    world: "plugin",
});

use formatter::HostExt;

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

    // This is the settings table an application would obtain from its own
    // configuration file. `ending` is deliberately omitted.
    let settings = serde_json::json!({ "prefix": "Configured" });
    let mut builder = HostBuilder::new(())?;
    let prepared = builder
        .prepare(
            PLUGIN_ID,
            &bytes,
            PluginConfig {
                settings: Some(settings),
                ..PluginConfig::default()
            },
        )
        .await?;
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await?;
    let host = builder.finish();

    let output = host
        .formatter(&plugin)?
        .format(InvocationCtx::bounded(25_000_000), "world")
        .await?;
    println!("{output}");
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
