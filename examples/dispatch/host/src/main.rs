use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lockgate::{Acceptance, HostBuilder, HostCtx, InvocationCtx, PluginConfig, RuntimeLimits};

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
    imports: Imports,
    data: (),
});

use tasks::HostExt;

impl dispatch::Host for Imports {
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
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await?;
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits {
                max_detached_jobs: 3,
                ..RuntimeLimits::default()
            },
            InvocationCtx::bounded(1_000_000),
        )
        .await?;
    let host = builder.finish();
    let tasks = host.tasks(&plugin)?;

    let index = tasks
        .run(InvocationCtx::bounded(25_000_000), "index")
        .await?;
    let backup = tasks
        .run(InvocationCtx::bounded(25_000_000), "backup")
        .await?;
    println!("plugin calls returned: {index}; {backup}");

    tokio::time::sleep(DELIVERY_WINDOW).await;
    tokio::task::spawn_blocking(move || drop(host)).await?;
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
