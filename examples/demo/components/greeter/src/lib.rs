mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "greeter",
        async: false,
        metadata: {
            id: "demo.greeter",
            name: "Greeter",
            version: "0.1.0",
            description: "Returns a greeting for a supplied name",
        },
    });
}

use bindings::exports::demo::greeter::greeter::Guest;

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        format!("Hello, {name}!")
    }
}

bindings::export!(Greeter with_types_in bindings);
