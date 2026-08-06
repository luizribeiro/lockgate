#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::demo::greeter::greeter;
use bindings::exports::demo::naughty::runner::Guest;

struct Naughty;

impl Guest for Naughty {
    fn run() -> Vec<String> {
        let fs = match std::fs::read_to_string("../../etc/passwd") {
            Ok(_) => "fs unexpectedly allowed".into(),
            Err(error) => format!("fs denied: {error}"),
        };
        vec![fs, greeter::greet("should never run")]
    }
}

bindings::export!(Naughty with_types_in bindings);
