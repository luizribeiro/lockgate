use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lockgate::{BudgetClass, HostBuilder, HostCtx, InvocationCtx, PluginConfig, RuntimeLimits};

const PLUGIN_ID: &str = "audit";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/audit/plugin/target/wasm32-wasip2/release/audit_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/audit/plugin/Cargo.toml --target wasm32-wasip2 --release";
const INVOCATION_DEADLINE: Duration = Duration::from_secs(30);

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
    imports: Imports,
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
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await?;
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            call("startup", 1_000_000),
        )
        .await?;
    let host = builder.finish();
    let tasks = host.tasks(&plugin)?;

    tasks.run(call("alice", 25_000_000), "index").await?;
    tasks.run(call("bob", 25_000_000), "backup").await?;

    for line in lines.lock().unwrap().iter() {
        println!("{line}");
    }
    Ok(())
}

fn call(user: &str, fuel: u64) -> InvocationCtx<CallOrigin> {
    InvocationCtx::new(
        CallOrigin { user: user.into() },
        BudgetClass::Bounded {
            fuel,
            deadline: INVOCATION_DEADLINE,
        },
    )
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
