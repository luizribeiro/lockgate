#[allow(warnings)]
mod bindings;

use bindings::exports::demo::caller::runner::Guest;
use bindings::tangent::core::registry::{self, Value};

struct Caller;

impl Guest for Caller {
    fn run() -> Result<String, String> {
        let target = "demo:greeter/greeter@0.1.0#greet";
        let handle = registry::lookup(target).map_err(|error| format!("lookup: {error:?}"))?;
        let values = registry::invoke(handle, &[Value::String("world".into())])
            .map_err(|error| format!("invoke: {error:?}"))?;
        match values.as_slice() {
            [Value::String(message)] => Ok(message.clone()),
            _ => Err("greeter returned an unexpected value".into()),
        }
    }
}

bindings::export!(Caller with_types_in bindings);
