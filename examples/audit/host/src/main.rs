use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lockgate::PluginId;
use lockgate::{HostBuilder, HostCtx, PluginConfig, RuntimeLimits};

const PLUGIN_ID: &str = "audit";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/audit/plugin/target/wasm32-wasip2/release/audit_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/audit/plugin/Cargo.toml --target wasm32-wasip2 --release";

#[derive(Clone)]
struct CallOrigin {
    user: String,
}

#[derive(Clone)]
struct Imports {
    lines: Arc<Mutex<Vec<String>>>,
}

lockgate::host_bindings!({
    path: "../wit",
    world: "plugin",
    imports_type: Imports,
    data: CallOrigin,
});

use tasks::HostExt;

#[lockgate::guarded]
impl audit::Host for Imports {
    #[lockgate::no_capability_required(
        reason = "demonstrates call-origin propagation in the audit example"
    )]
    async fn log(&mut self, cx: HostCtx<'_, CallOrigin>, message: String) {
        self.lines
            .lock()
            .unwrap()
            .push(format!("{}: {message}", cx.data().user));
    }
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

    let lines = Arc::new(Mutex::new(Vec::new()));
    let imports = Imports {
        lines: Arc::clone(&lines),
    };
    let mut builder = HostBuilder::new(imports)?;
    let prepared = builder
        .prepare(
            PluginId::try_from(PLUGIN_ID).unwrap(),
            &bytes,
            PluginConfig::default(),
        )
        .await?;
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit_with_data(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            call("startup"),
        )
        .await?;
    let host = builder.finish();
    let tasks = host.tasks(&plugin)?;

    tasks.run(call("alice"), "index").await?;
    tasks.run(call("bob"), "backup").await?;

    for line in lines.lock().unwrap().iter() {
        println!("{line}");
    }
    host.shutdown().await;
    Ok(())
}

fn call(user: &str) -> CallOrigin {
    CallOrigin { user: user.into() }
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
