//! Adapts the typed agent contract to Inkling's OpenAI-compatible wire format.

mod bindings {
    lockgate_plugin::bindings!({
        path: "../../wit",
        world: "provider",
        metadata: {
            id: "coding.provider.inkling",
            name: "Inkling",
            version: "0.1.0",
            description: "Calls an Inkling OpenAI-compatible model server",
        },
    });
}

use bindings::coding::agent::{http_client, provider_settings};
use bindings::exports::coding::agent::provider::{
    Completion, Conversation, Guest, GuestConversation, ToolCall, ToolResult, ToolSpec,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080/v1";
const DEFAULT_MODEL: &str = "Inkling-Small-mxfp4";
const DEFAULT_MAX_TOKENS: u32 = 2_048;

struct InklingProvider;

impl Guest for InklingProvider {
    type Conversation = InklingConversation;

    fn open(system_prompt: String, tools: Vec<ToolSpec>) -> Result<Conversation, String> {
        Ok(Conversation::new(InklingConversation {
            messages: RefCell::new(vec![WireMessage::system(system_prompt)]),
            tools: tools
                .into_iter()
                .map(WireTool::try_from)
                .collect::<Result<_, _>>()?,
        }))
    }
}

struct InklingConversation {
    messages: RefCell<Vec<WireMessage>>,
    tools: Vec<WireTool>,
}

impl GuestConversation for InklingConversation {
    fn send(&self, prompt: String) -> Result<Completion, String> {
        self.messages.borrow_mut().push(WireMessage::user(prompt));
        self.complete()
    }

    fn resume(&self, results: Vec<ToolResult>) -> Result<Completion, String> {
        self.messages
            .borrow_mut()
            .extend(results.into_iter().map(WireMessage::tool));
        self.complete()
    }
}

impl InklingConversation {
    fn complete(&self) -> Result<Completion, String> {
        let base_url = setting("base-url").unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = setting("model").unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let max_tokens = setting("max-tokens")
            .map(|value| {
                value
                    .parse()
                    .map_err(|error| format!("invalid max-tokens setting: {error}"))
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_TOKENS);
        let request = Request {
            model,
            messages: self.messages.borrow().clone(),
            tools: self.tools.clone(),
            max_tokens,
            stream: false,
        };
        let request = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        let response = http_client::post(
            &format!("{}/chat/completions", base_url.trim_end_matches('/')),
            &[http_client::Header {
                name: "content-type".to_owned(),
                value: "application/json".to_owned(),
            }],
            &request,
        )?;
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "Inkling returned HTTP {}: {}",
                response.status,
                response.body.chars().take(500).collect::<String>()
            ));
        }
        let response: Response = serde_json::from_str(&response.body)
            .map_err(|error| format!("invalid Inkling response: {error}"))?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "Inkling returned no completion choices".to_owned())?;
        let message = choice.message;
        let content = message.content.clone();
        let tool_calls = message.tool_calls.clone();
        self.messages
            .borrow_mut()
            .push(WireMessage::assistant(message));
        Ok(Completion {
            content,
            tool_calls: tool_calls.into_iter().map(ToolCall::from).collect(),
        })
    }
}

fn setting(key: &str) -> Option<String> {
    provider_settings::get(key)
}

#[derive(Serialize)]
struct Request {
    model: String,
    messages: Vec<WireMessage>,
    tools: Vec<WireTool>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Clone, Serialize)]
struct WireMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
}

impl WireMessage {
    fn system(content: String) -> Self {
        Self {
            role: "system".to_owned(),
            content: Some(content),
            reasoning_content: None,
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    fn user(content: String) -> Self {
        Self {
            role: "user".to_owned(),
            content: Some(content),
            reasoning_content: None,
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    fn tool(result: ToolResult) -> Self {
        Self {
            role: "tool".to_owned(),
            content: Some(result.content),
            reasoning_content: None,
            name: Some(result.name),
            tool_call_id: Some(result.call_id),
            tool_calls: Vec::new(),
        }
    }

    fn assistant(message: ResponseMessage) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: message.content,
            reasoning_content: message.reasoning_content,
            name: None,
            tool_call_id: None,
            tool_calls: message.tool_calls,
        }
    }
}

#[derive(Clone, Serialize)]
struct WireTool {
    r#type: &'static str,
    function: WireFunctionSpec,
}

#[derive(Clone, Serialize)]
struct WireFunctionSpec {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl TryFrom<ToolSpec> for WireTool {
    type Error = String;

    fn try_from(spec: ToolSpec) -> Result<Self, Self::Error> {
        let parameters = serde_json::from_str(&spec.input_schema_json)
            .map_err(|error| format!("tool `{}` supplied an invalid schema: {error}", spec.name))?;
        Ok(Self {
            r#type: "function",
            function: WireFunctionSpec {
                name: spec.name,
                description: spec.description,
                parameters,
            },
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct WireToolCall {
    id: String,
    r#type: String,
    function: WireFunctionCall,
}

#[derive(Clone, Deserialize, Serialize)]
struct WireFunctionCall {
    name: String,
    arguments: String,
}

impl From<ToolCall> for WireToolCall {
    fn from(call: ToolCall) -> Self {
        Self {
            id: call.id,
            r#type: "function".to_owned(),
            function: WireFunctionCall {
                name: call.name,
                arguments: call.arguments_json,
            },
        }
    }
}

impl From<WireToolCall> for ToolCall {
    fn from(call: WireToolCall) -> Self {
        Self {
            id: call.id,
            name: call.function.name,
            arguments_json: call.function.arguments,
        }
    }
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

bindings::export!(InklingProvider with_types_in bindings);
