#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::exports::demo::dynamic::runner::Guest;
use bindings::lockgate::core::registry::{self, Value};

struct Dynamic;

impl Guest for Dynamic {
    fn run() -> Vec<String> {
        let target = ["demo:greeter/greeter@0.1.0", "greet"].join("#");
        let allowed = registry::lookup(&target)
            .and_then(|handle| registry::invoke(handle, &[Value::String("dynamic".into())]));
        let allowed = match allowed {
            Ok(values) => match values.as_slice() {
                [Value::String(message)] => format!("allowed: {message}"),
                _ => format!("allowed call returned unexpected values: {values:?}"),
            },
            Err(error) => format!("allowed call failed: {error:?}"),
        };
        let denied = match registry::lookup("demo:greeter/greeter@0.1.0#missing") {
            Ok(_) => "denied lookup unexpectedly allowed".into(),
            Err(error) => format!("denied: {error:?}"),
        };
        vec![allowed, denied]
    }
}

bindings::export!(Dynamic with_types_in bindings);
