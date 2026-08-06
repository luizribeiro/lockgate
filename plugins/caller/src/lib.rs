#[allow(warnings)]
mod bindings;

use bindings::exports::demo::caller::runner::Guest;
use bindings::demo::greeter::greeter;

struct Caller;

impl Guest for Caller {
    fn run() -> Result<String, String> {
        Ok(greeter::greet("world"))
    }
}

bindings::export!(Caller with_types_in bindings);
