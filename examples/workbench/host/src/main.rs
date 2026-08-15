use std::error::Error;
use std::path::{Path, PathBuf};

use lockgate::{
    Acceptance, Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, RoleError,
    RuntimeLimits,
};

const TIDY_ID: &str = "tidy";
const COUNTER_ID: &str = "counter";
const DEFAULT_TIDY_PATH: &str =
    "examples/workbench/plugins/tidy/target/wasm32-wasip2/release/tidy_plugin.wasm";
const DEFAULT_COUNTER_PATH: &str =
    "examples/workbench/plugins/counter/target/wasm32-wasip2/release/counter_plugin.wasm";
const TIDY_BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/workbench/plugins/tidy/Cargo.toml --target wasm32-wasip2 --release";
const COUNTER_BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/workbench/plugins/counter/Cargo.toml --target wasm32-wasip2 --release";
const SAMPLE_TEXT: &str = "  alpha   beta\ngamma   \n";
const CALL_FUEL: u64 = 25_000_000;

lockgate::host_bindings!({
    path: "../wit",
    world: "host",
});

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let tidy_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TIDY_PATH));
    let counter_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_COUNTER_PATH));
    let tidy_bytes = read_component(&tidy_path, TIDY_BUILD_COMMAND)?;
    let counter_bytes = read_component(&counter_path, COUNTER_BUILD_COMMAND)?;

    let mut builder = HostBuilder::new(())?;
    let tidy = builder
        .prepare(TIDY_ID, &tidy_bytes, PluginConfig::default())
        .await?;
    let counter = builder
        .prepare(COUNTER_ID, &counter_bytes, PluginConfig::default())
        .await?;
    builder
        .admit(
            tidy,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await?;
    builder
        .admit(
            counter,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await?;
    let host = builder.finish();

    for plugin in host.plugins() {
        run_formatter(&host, plugin).await?;
        run_linter(&host, plugin).await?;
        run_stats(&host, plugin).await?;
    }
    Ok(())
}

async fn run_formatter(host: &Host<()>, plugin: &PluginHandle) -> Result<(), Box<dyn Error>> {
    match formatter::HostExt::formatter(host, plugin) {
        Ok(formatter) => {
            let result = formatter
                .format(InvocationCtx::bounded(CALL_FUEL), SAMPLE_TEXT)
                .await?;
            println!("{} formatter: {result}", plugin.id());
        }
        Err(RoleError::RoleNotExported { .. }) => {
            println!("{} formatter: not implemented", plugin.id());
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn run_linter(host: &Host<()>, plugin: &PluginHandle) -> Result<(), Box<dyn Error>> {
    match linter::HostExt::linter(host, plugin) {
        Ok(linter) => {
            let findings = linter
                .lint(InvocationCtx::bounded(CALL_FUEL), SAMPLE_TEXT)
                .await?;
            let result = if findings.is_empty() {
                "no findings".to_owned()
            } else {
                findings.join(", ")
            };
            println!("{} linter: {result}", plugin.id());
        }
        Err(RoleError::RoleNotExported { .. }) => {
            println!("{} linter: not implemented", plugin.id());
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn run_stats(host: &Host<()>, plugin: &PluginHandle) -> Result<(), Box<dyn Error>> {
    match stats::HostExt::stats(host, plugin) {
        Ok(stats) => {
            let result = stats
                .measure(InvocationCtx::bounded(CALL_FUEL), SAMPLE_TEXT)
                .await?;
            println!("{} stats: {result}", plugin.id());
        }
        Err(RoleError::RoleNotExported { .. }) => {
            println!("{} stats: not implemented", plugin.id());
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn read_component(path: &Path, build_command: &str) -> Result<Vec<u8>, std::io::Error> {
    std::fs::read(path).map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "could not read plugin component at `{}`: {source}\n\
                 build the plugin first with:\n  {build_command}",
                path.display()
            ),
        )
    })
}
