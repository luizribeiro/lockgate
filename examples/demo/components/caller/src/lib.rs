mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "caller",
        metadata: {
            id: "demo.caller",
            name: "Caller",
            version: "0.1.0",
            description: "Invokes a linked greeter plugin",
        },
    });
}

use bindings::demo::greeter::greeter;
use bindings::demo::host::services;
use bindings::exports::demo::host::runnable::Guest;

struct Caller;

impl Guest for Caller {
    async fn run() -> Result<String, String> {
        services::log("caller is invoking its greeter sibling".to_owned()).await;
        Ok(greeter::greet("world".to_owned()).await)
    }
}

bindings::export!(Caller with_types_in bindings);
