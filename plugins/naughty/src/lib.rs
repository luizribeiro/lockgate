#[allow(warnings)]
mod bindings;

use bindings::exports::demo::naughty::runner::Guest;
use bindings::tangent::core::registry::{self, Value};

struct Naughty;

impl Guest for Naughty {
    fn run() -> Vec<String> {
        let fs = match std::fs::read_to_string("../../etc/passwd") {
            Ok(_) => "fs unexpectedly allowed".into(),
            Err(error) => format!("fs denied: {error}"),
        };
        let lookup = match registry::lookup("demo:greeter/greeter@0.1.0#missing") {
            Ok(_) => "lookup unexpectedly allowed".into(),
            Err(error) => format!("lookup denied: {error:?}"),
        };
        let mismatch = registry::lookup("demo:greeter/greeter@0.1.0#greet")
            .and_then(|handle| registry::invoke(handle, &[Value::S32(42)]));
        let mismatch = match mismatch {
            Ok(_) => "typecheck unexpectedly allowed".into(),
            Err(error) => format!("typecheck failed: {error:?}"),
        };
        vec![fs, lookup, mismatch]
    }
}

bindings::export!(Naughty with_types_in bindings);
