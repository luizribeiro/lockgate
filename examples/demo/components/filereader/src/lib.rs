#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::exports::demo::filereader::runner::Guest;

struct FileReader;

impl Guest for FileReader {
    fn run() -> Result<String, String> {
        let contents = std::fs::read_to_string("/shared/allowed.txt")
            .map_err(|error| error.to_string())?;
        Ok(contents.lines().next().unwrap_or_default().to_owned())
    }
}

bindings::export!(FileReader with_types_in bindings);
