//! A tool plugin with only the host capability needed to read workspace files.

mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "tool",
        metadata: {
            id: "coding.tool.read-file",
            name: "Read File Tool",
            version: "0.1.0",
            description: "Reads one UTF-8 file beneath the granted workspace",
        },
    });
}

use bindings::coding::agent::workspace;
use bindings::exports::coding::agent::tool::Guest;
use serde::Deserialize;

#[derive(Deserialize)]
struct Arguments {
    path: String,
}

struct ReadFile;

impl Guest for ReadFile {
    async fn name() -> String {
        "read_file".to_owned()
    }

    async fn description() -> String {
        "Read a UTF-8 file using a path relative to the workspace".to_owned()
    }

    async fn input_schema() -> String {
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false
        })
        .to_string()
    }

    async fn run(arguments_json: String) -> Result<String, String> {
        let arguments: Arguments = serde_json::from_str(&arguments_json)
            .map_err(|error| format!("invalid arguments: {error}"))?;
        workspace::read_file(arguments.path).await
    }
}

bindings::export!(ReadFile with_types_in bindings);
