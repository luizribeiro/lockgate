use std::error::Error;
use std::path::{Path, PathBuf};

use lockgate::PluginId;
use lockgate::{HostBuilder, PluginConfig, RoleError, RuntimeLimits};

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
        .prepare(
            PluginId::try_from(TIDY_ID).unwrap(),
            &tidy_bytes,
            PluginConfig::default(),
        )
        .await?;
    let counter = builder
        .prepare(
            PluginId::try_from(COUNTER_ID).unwrap(),
            &counter_bytes,
            PluginConfig::default(),
        )
        .await?;
    let tidy_acceptance = tidy.accept_all();
    let counter_acceptance = counter.accept_all();
    builder
        .admit(tidy, tidy_acceptance, RuntimeLimits::default())
        .await?;
    builder
        .admit(counter, counter_acceptance, RuntimeLimits::default())
        .await?;
    let host = builder.finish();

    for plugin in host.plugins() {
        let formatter = role_status(formatter::HostExt::formatter(&host, plugin))?;
        let linter = role_status(linter::HostExt::linter(&host, plugin))?;
        let stats = role_status(stats::HostExt::stats(&host, plugin))?;
        println!(
            "{} roles: formatter={formatter}, linter={linter}, stats={stats}",
            plugin.id()
        );
    }

    for (plugin, formatter) in formatter::HostExt::formatter_clients(&host) {
        let result = formatter.format(SAMPLE_TEXT).await?;
        println!("formatter fan-out: {} -> {result}", plugin.id());
    }
    for (plugin, linter) in linter::HostExt::linter_clients(&host) {
        let findings = linter.lint(SAMPLE_TEXT).await?;
        let result = if findings.is_empty() {
            "no findings".to_owned()
        } else {
            findings.join(", ")
        };
        println!("linter fan-out: {} -> {result}", plugin.id());
    }
    for (plugin, stats) in stats::HostExt::stats_clients(&host) {
        let result = stats.measure(SAMPLE_TEXT).await?;
        println!("stats fan-out: {} -> {result}", plugin.id());
    }
    host.shutdown().await;
    Ok(())
}

fn role_status<T>(cast: Result<T, RoleError>) -> Result<&'static str, RoleError> {
    match cast {
        Ok(_) => Ok("implemented"),
        Err(RoleError::RoleNotExported { .. }) => Ok("not implemented"),
        Err(error) => Err(error),
    }
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
