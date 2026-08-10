//! A tool plugin with only the host capability needed to enumerate workspace files.

mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "tool",
        async: false,
        metadata: {
            id: "coding.tool.list-files",
            name: "List Files Tool",
            version: "0.1.0",
            description: "Lists files beneath the granted workspace",
        },
    });
}

use bindings::coding::agent::workspace;
use bindings::exports::coding::agent::tool::Guest;

struct ListFiles;

impl Guest for ListFiles {
    fn name() -> String {
        "list_files".to_owned()
    }

    fn description() -> String {
        "List relative paths in the current workspace".to_owned()
    }

    fn input_schema() -> String {
        serde_json::json!({ "type": "object", "properties": {} }).to_string()
    }

    fn run(_arguments_json: String) -> Result<String, String> {
        workspace::list_files().map(|files| files.join("\n"))
    }
}

bindings::export!(ListFiles with_types_in bindings);
