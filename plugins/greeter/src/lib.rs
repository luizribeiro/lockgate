#[allow(warnings)]
mod bindings;

use bindings::exports::demo::greeter::greeter::Guest;

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        if name == "__registry_probe__" {
            let _ = bindings::tangent::core::registry::lookup("probe:unused/x@0.1.0#x");
        }
        format!("Hello, {name}!")
    }
}

bindings::export!(Greeter with_types_in bindings);
