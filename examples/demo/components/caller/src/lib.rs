#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::demo::greeter::greeter;
use bindings::exports::demo::caller::runner::Guest;

struct Caller;

impl Guest for Caller {
    fn run() -> Result<String, String> {
        Ok(greeter::greet("world"))
    }
}

bindings::export!(Caller with_types_in bindings);
