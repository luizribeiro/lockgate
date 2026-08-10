mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "filereader",
        metadata: {
            id: "demo.filereader",
            name: "File Reader",
            version: "0.1.0",
            description: "Reads files granted by the embedding application",
        },
    });
}

use bindings::demo::host::services;
use bindings::exports::demo::host::{file_reader, runnable};

struct FileReader;

impl file_reader::Guest for FileReader {
    async fn read(path: String) -> Result<String, String> {
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Ok(contents.lines().next().unwrap_or_default().to_owned())
    }
}

impl runnable::Guest for FileReader {
    async fn run() -> Result<String, String> {
        services::log("filereader is reading its preopened directory".to_owned()).await;
        <Self as file_reader::Guest>::read("/shared/allowed.txt".into()).await
    }
}

bindings::export!(FileReader with_types_in bindings);
