#[allow(warnings)]
mod bindings;

use bindings::exports::demo::filereader::runner::Guest;

struct FileReader;

impl Guest for FileReader {
    fn run() -> Result<String, String> {
        if std::env::var_os("REGISTRY_PROBE").is_some() {
            let _ = bindings::tangent::core::registry::lookup("probe:unused/x@0.1.0#x");
        }
        let contents = std::fs::read_to_string("/shared/allowed.txt")
            .map_err(|error| error.to_string())?;
        Ok(contents.lines().next().unwrap_or_default().to_owned())
    }
}

bindings::export!(FileReader with_types_in bindings);
