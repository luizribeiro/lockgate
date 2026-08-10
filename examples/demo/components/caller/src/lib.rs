mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "caller",
        async: false,
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
    fn run() -> Result<String, String> {
        services::log("caller is invoking its greeter sibling");
        Ok(greeter::greet("world"))
    }
}

bindings::export!(Caller with_types_in bindings);
