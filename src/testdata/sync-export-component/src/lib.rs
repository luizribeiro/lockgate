mod bindings {
    lockgate_plugin::bindings!({
        path: "wit",
        world: "plugin",
        metadata: {
            id: "sync-export",
            name: "Synchronous export fixture",
            version: "0.0.0",
            description: "Exercises synchronous host invocation",
        },
    });
}

use bindings::exports::demo::sync_exports::values::Guest;

struct Fixture;

impl Guest for Fixture {
    fn answer() -> Result<String, String> {
        Ok("hello world".to_owned())
    }
}

bindings::export!(Fixture with_types_in bindings);
