//! Host-owned conversation values and provider-neutral WIT conversions.

use crate::bindings::__lockgate_world_0::exports::coding::agent::provider as wit;

#[derive(Clone, Debug)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
}

impl ToolCall {
    pub(crate) fn from_wit(call: wit::ToolCall) -> Self {
        Self {
            id: call.id,
            name: call.name,
            arguments: call.arguments_json,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Message {
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) name: Option<String>,
    pub(crate) tool_calls: Vec<ToolCall>,
}

impl Message {
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self::plain("system", content)
    }

    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self::plain("user", content)
    }

    pub(crate) fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::plain("assistant", content)
        }
    }

    pub(crate) fn tool(call: &ToolCall, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_owned(),
            content: content.into(),
            name: Some(call.name.clone()),
            tool_calls: Vec::new(),
        }
    }

    fn plain(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: content.into(),
            name: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ToolSpec {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema_json: String,
}

impl ToolSpec {
    pub(crate) fn to_wit(&self) -> wit::ToolSpec {
        wit::ToolSpec {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema_json: self.input_schema_json.clone(),
        }
    }
}
