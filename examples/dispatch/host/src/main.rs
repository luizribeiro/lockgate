use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lockgate::PluginId;
use lockgate::{HostBuilder, HostCtx, PluginConfig, RuntimeLimits};

const PLUGIN_ID: &str = "dispatch";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/dispatch/plugin/target/wasm32-wasip2/release/dispatch_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/dispatch/plugin/Cargo.toml --target wasm32-wasip2 --release";
const INDEX_DELAY: Duration = Duration::from_millis(250);
const BACKUP_DELAY: Duration = Duration::from_millis(350);
const FAILURE_DELAY: Duration = Duration::from_millis(450);
const DELIVERY_WINDOW: Duration = Duration::from_millis(600);

#[derive(Clone, Copy)]
struct Imports;

lockgate::host_bindings!({
    path: "../wit",
    world: "plugin",
    imports_type: Imports,
    data: (),
});

use tasks::HostExt;

#[lockgate::guarded]
impl dispatch::Host for Imports {
    #[lockgate::no_capability_required(
        reason = "demonstrates detached host work in the dispatch example"
    )]
    async fn send(&mut self, cx: HostCtx<'_, ()>, report: String) {
        let delay = match report.as_str() {
            "index report" => INDEX_DELAY,
            "backup report" => BACKUP_DELAY,
            _ => FAILURE_DELAY,
        };
        cx.detach(deliver(report, delay))
            .expect("the example's detached-job quota should accommodate every report");
    }
}

async fn deliver(report: String, delay: Duration) -> Result<(), std::io::Error> {
    tokio::time::sleep(delay).await;
    if report == "undeliverable report" {
        return Err(std::io::Error::other(format!("could not deliver {report}")));
    }
    println!("delivered: {report}");
    Ok(())
}

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

    let mut builder = HostBuilder::new(Imports)?;
    builder.on_detached_job_error(|failure| {
        println!(
            "detached job failed: plugin={} job={} error={}",
            failure.plugin_id(),
            failure.job_id(),
            failure.error()
        );
    });
    let prepared = builder
        .prepare(
            PluginId::try_from(PLUGIN_ID).unwrap(),
            &bytes,
            PluginConfig::default(),
        )
        .await?;
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits {
                max_detached_jobs: 3,
                ..RuntimeLimits::default()
            },
        )
        .await?;
    let host = builder.finish();
    let tasks = host.tasks(&plugin)?;

    let index = tasks.run("index").await?;
    let backup = tasks.run("backup").await?;
    println!("plugin calls returned: {index}; {backup}");

    tokio::time::sleep(DELIVERY_WINDOW).await;
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
