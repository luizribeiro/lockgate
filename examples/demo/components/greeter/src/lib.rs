mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "greeter",
        generate_all,
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
