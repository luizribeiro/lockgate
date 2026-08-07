#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::demo::host::services;
use bindings::exports::demo::host::{file_reader, runnable};

struct FileReader;

impl file_reader::Guest for FileReader {
    fn read(path: String) -> Result<String, String> {
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Ok(contents.lines().next().unwrap_or_default().to_owned())
    }

}

impl runnable::Guest for FileReader {
    fn run() -> Result<String, String> {
        services::log("filereader is reading its preopened directory");
        <Self as file_reader::Guest>::read("/shared/allowed.txt".into())
    }
}

bindings::export!(FileReader with_types_in bindings);
