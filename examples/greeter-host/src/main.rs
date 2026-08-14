use std::error::Error;
use std::path::{Path, PathBuf};

use lockgate::{
    Acceptance, CallError, HostBuilder, InvocationCtx, PluginConfig, Role, RoleInvocation,
    RuntimeLimits, Value,
};

const PLUGIN_ID: &str = "example.greeter";
const GREETER_INTERFACE: &str = "example:greeter/greeter";
const DEFAULT_PLUGIN_PATH: &str =
    "examples/greeter-plugin/target/wasm32-wasip2/release/greeter_plugin.wasm";
const BUILD_COMMAND: &str = "nix develop -c cargo build --manifest-path \
examples/greeter-plugin/Cargo.toml --target wasm32-wasip2 --release";

struct GreeterRole;

struct GreeterClient<'a, S: Send + 'static> {
    invocation: RoleInvocation<'a, S>,
}

impl Role for GreeterRole {
    const INTERFACE: &'static str = GREETER_INTERFACE;
    type Client<'a, S>
        = GreeterClient<'a, S>
    where
        S: Send + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + 'static,
    {
        GreeterClient { invocation }
    }
}

impl<S: Send + 'static> GreeterClient<'_, S> {
    async fn greet(&self, name: &str, ctx: InvocationCtx<S>) -> Result<String, CallError> {
        let results = self
            .invocation
            .invoke("greet", &[Value::String(name.to_owned())], ctx)
            .await?;
        match results.as_slice() {
            [Value::String(greeting)] => Ok(greeting.clone()),
            _ => Err(CallError::shape("greet returned a non-string result")),
        }
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

    let mut builder = HostBuilder::<()>::new()?;
    // prepare validates the declared identity, needs, and supported WIT shape.
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await?;
    // admit records consent, applies limits, and proves the component can start.
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await?;
    let host = builder.finish();

    let greeter = host.client::<GreeterRole>(&plugin)?;
    // Every call receives its own application context and execution budget.
    let greeting = greeter
        .greet("world", InvocationCtx::bounded(25_000_000))
        .await?;
    println!("{greeting}");
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
