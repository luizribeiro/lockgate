#[allow(unsafe_op_in_unsafe_fn)]
mod bindings;

use bindings::exports::demo::filereader::runner::Guest;

struct FileReader;

impl Guest for FileReader {
    fn read(path: String) -> Result<String, String> {
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Ok(contents.lines().next().unwrap_or_default().to_owned())
    }

    fn run() -> Result<String, String> {
        Self::read("/shared/allowed.txt".into())
    }
}

bindings::export!(FileReader with_types_in bindings);
