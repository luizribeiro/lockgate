//! A small TUI coding agent assembled from a provider component and tool components.

use anyhow::{Context, Result, anyhow, bail};
use lockgate::{Application, Component, HostContext, Runtime};
use protocol::{Message, ToolCall, ToolSpec};
use std::{env, path::PathBuf};

mod artifacts;
mod http;
mod protocol;
mod settings;
mod ui;
mod workspace;

mod bindings {
    lockgate::bindings! {
        path: "wit/deps/coding-agent",
        worlds: {
            ProviderPlugin: "provider-plugin",
            ToolPlugin: "tool-plugin",
        },
    }
}

const WORKSPACE_INTERFACE: &str = "coding:agent/workspace@0.1.0";
const HTTP_CLIENT_INTERFACE: &str = "coding:agent/http-client@0.1.0";
const PROVIDER_SETTINGS_INTERFACE: &str = "coding:agent/provider-settings@0.1.0";
const MAX_AGENT_STEPS: usize = 8;
const PLUGIN_FUEL_PER_CALL: u64 = 25_000_000;

type ProviderConversation =
    bindings::__lockgate_world_0::exports::coding::agent::provider::Conversation;

struct AppState {
    workspace: workspace::Workspace,
    http: http::Client,
    provider_settings: settings::Settings,
}

struct ToolRegistration {
    component: Component<bindings::ToolPlugin>,
    spec: ToolSpec,
}

impl bindings::coding::agent::workspace::Host for HostContext<AppState> {}

impl bindings::coding::agent::workspace::HostWithStore<lockgate::__private::PluginStore<AppState>>
    for HostContext<AppState>
{
    async fn list_files(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
    ) -> Result<Vec<String>, String> {
        accessor.with(|mut access| access.get().state().workspace.list_files())
    }

    async fn read_file(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
        path: String,
    ) -> Result<String, String> {
        accessor.with(|mut access| access.get().state().workspace.read_file(&path))
    }
}

impl bindings::coding::agent::http_client::Host for HostContext<AppState> {}

impl bindings::coding::agent::http_client::HostWithStore<lockgate::__private::PluginStore<AppState>>
    for HostContext<AppState>
{
    async fn post(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
        url: String,
        headers: Vec<bindings::coding::agent::http_client::Header>,
        body: String,
    ) -> Result<bindings::coding::agent::http_client::Response, String> {
        let http = accessor.with(|mut access| access.get().state().http.clone());
        let response = http
            .post(
                url,
                headers
                    .into_iter()
                    .map(|header| (header.name, header.value))
                    .collect(),
                body,
            )
            .await?;
        Ok(bindings::coding::agent::http_client::Response {
            status: response.status,
            body: response.body,
        })
    }
}

impl bindings::coding::agent::provider_settings::Host for HostContext<AppState> {}

impl
    bindings::coding::agent::provider_settings::HostWithStore<
        lockgate::__private::PluginStore<AppState>,
    > for HostContext<AppState>
{
    async fn get(
        accessor: &wasmtime::component::Accessor<lockgate::__private::PluginStore<AppState>, Self>,
        key: String,
    ) -> Option<String> {
        accessor.with(|mut access| access.get().state().provider_settings.get(&key))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace = workspace::Workspace::new(root)?;
    let workspace_label = workspace.display();

    let state = AppState {
        workspace,
        http: http::Client::from_env()?,
        provider_settings: settings::Settings::from_env(),
    };
    let mut app = Application::new(state)?.fuel_per_call(PLUGIN_FUEL_PER_CALL);
    let provider = app.add::<bindings::ProviderPlugin>(artifacts::provider_bytes()?)?;
    let list_files = app.add::<bindings::ToolPlugin>(artifacts::component_bytes("list-files")?)?;
    let read_file = app.add::<bindings::ToolPlugin>(artifacts::component_bytes("read-file")?)?;
    let provider_name = app.metadata(provider)?.name().to_owned();
    let runtime = app
        .allow_host_import(provider, HTTP_CLIENT_INTERFACE)?
        .allow_host_import(provider, PROVIDER_SETTINGS_INTERFACE)?
        .allow_host_import(list_files, WORKSPACE_INTERFACE)?
        .allow_host_import(read_file, WORKSPACE_INTERFACE)?
        .run()
        .await?;
    let tools = register_tools(&runtime, [list_files, read_file]).await?;

    let system_prompt =
        "You are a concise coding agent. Inspect the workspace with tools before making claims.";
    let tool_specs = tools
        .iter()
        .map(|tool| tool.spec.to_wit())
        .collect::<Vec<_>>();
    let conversation = runtime
        .component(provider)
        .open(system_prompt.to_owned(), tool_specs)
        .await?
        .map_err(|error| anyhow!("provider: {error}"))?;
    let mut messages = vec![Message::system(system_prompt)];
    let mut ui = ui::Ui::new()?;
    let ready = format!("ready • provider: {provider_name} • {} tools", tools.len());

    loop {
        ui.draw(&messages, &ready, &workspace_label)?;
        match ui.next_action()? {
            ui::Action::Quit => break,
            ui::Action::None => {}
            ui::Action::Submit(prompt) => {
                messages.push(Message::user(prompt.clone()));
                ui.draw(&messages, "thinking…", &workspace_label)?;
                if let Err(error) = run_agent(
                    &runtime,
                    provider,
                    &conversation,
                    &tools,
                    &mut messages,
                    prompt,
                )
                .await
                {
                    messages.push(Message::assistant(format!("Error: {error:#}"), Vec::new()));
                }
            }
        }
    }
    Ok(())
}

async fn register_tools(
    runtime: &Runtime<AppState>,
    components: impl IntoIterator<Item = Component<bindings::ToolPlugin>>,
) -> Result<Vec<ToolRegistration>> {
    let mut tools = Vec::new();
    for component in components {
        let client = runtime.component(component);
        let name = client.name().await?;
        let description = client.description().await?;
        let schema = client.input_schema().await?;
        let _: serde_json::Value = serde_json::from_str(&schema)
            .with_context(|| format!("tool `{name}` returned an invalid JSON schema"))?;
        if tools
            .iter()
            .any(|tool: &ToolRegistration| tool.spec.name == name)
        {
            bail!("duplicate tool name `{name}`");
        }
        tools.push(ToolRegistration {
            component,
            spec: ToolSpec {
                name,
                description,
                input_schema_json: schema,
            },
        });
    }
    Ok(tools)
}

async fn run_agent(
    runtime: &Runtime<AppState>,
    provider: Component<bindings::ProviderPlugin>,
    conversation: &ProviderConversation,
    tools: &[ToolRegistration],
    messages: &mut Vec<Message>,
    prompt: String,
) -> Result<()> {
    let client = runtime.component(provider);
    let mut completion = client
        .conversation()
        .send(*conversation, prompt)
        .await?
        .map_err(|error| anyhow!("provider: {error}"))?;
    for step in 0..MAX_AGENT_STEPS {
        let calls = completion
            .tool_calls
            .into_iter()
            .map(ToolCall::from_wit)
            .collect::<Vec<_>>();
        messages.push(Message::assistant(
            completion.content.unwrap_or_default(),
            calls.clone(),
        ));
        if calls.is_empty() {
            return Ok(());
        }
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            let result = match tools
                .iter()
                .find(|registration| registration.spec.name == call.name)
            {
                Some(tool) => runtime
                    .component(tool.component)
                    .run(call.arguments.clone())
                    .await?
                    .unwrap_or_else(|error| format!("tool error: {error}")),
                None => format!("tool error: unknown tool `{}`", call.name),
            };
            messages.push(Message::tool(&call, result.clone()));
            results.push(
                bindings::__lockgate_world_0::exports::coding::agent::provider::ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: result,
                },
            );
        }
        if step + 1 == MAX_AGENT_STEPS {
            break;
        }
        completion = client
            .conversation()
            .resume(*conversation, results)
            .await?
            .map_err(|error| anyhow!("provider: {error}"))?;
    }
    bail!("agent exceeded the {MAX_AGENT_STEPS}-step limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    #[tokio::test]
    async fn inkling_provider_round_trips_a_sandboxed_tool_call() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(serve_inkling(listener));
        let workspace = workspace::Workspace::new(env!("CARGO_MANIFEST_DIR"))?;
        let state = AppState {
            workspace,
            http: http::Client::new([format!("http://{address}")])?,
            provider_settings: settings::Settings::new([
                ("base-url".to_owned(), format!("http://{address}/v1")),
                ("model".to_owned(), "Inkling-Small-mxfp4".to_owned()),
                ("max-tokens".to_owned(), "2048".to_owned()),
            ]),
        };
        let mut app = Application::new(state)?.fuel_per_call(PLUGIN_FUEL_PER_CALL);
        let provider =
            app.add::<bindings::ProviderPlugin>(artifacts::component_bytes("inkling-provider")?)?;
        let read_file =
            app.add::<bindings::ToolPlugin>(artifacts::component_bytes("read-file")?)?;
        let runtime = app
            .allow_host_import(provider, HTTP_CLIENT_INTERFACE)?
            .allow_host_import(provider, PROVIDER_SETTINGS_INTERFACE)?
            .allow_host_import(read_file, WORKSPACE_INTERFACE)?
            .run()
            .await?;
        let tools = register_tools(&runtime, [read_file]).await?;
        let tool_specs = tools
            .iter()
            .map(|tool| tool.spec.to_wit())
            .collect::<Vec<_>>();
        let conversation = runtime
            .component(provider)
            .open("test".to_owned(), tool_specs)
            .await?
            .map_err(|error| anyhow!("provider: {error}"))?;
        let prompt = "Read Cargo.toml and tell me the package name.";
        let mut messages = vec![Message::system("test"), Message::user(prompt)];

        run_agent(
            &runtime,
            provider,
            &conversation,
            &tools,
            &mut messages,
            prompt.to_owned(),
        )
        .await?;
        let requests = server.await??;

        assert!(messages.iter().any(|message| {
            message.role == "tool" && message.content.contains("lockgate-coding-agent")
        }));
        assert!(messages.last().is_some_and(|message| {
            message.role == "assistant" && message.content.contains("lockgate-coding-agent")
        }));
        assert_eq!(requests[0]["model"], "Inkling-Small-mxfp4");
        assert_eq!(requests[0]["tools"][0]["function"]["name"], "read_file");
        assert_eq!(
            requests[1]["messages"][2]["reasoning_content"],
            "I should inspect the manifest."
        );
        assert!(
            requests[1]["messages"][3]["content"]
                .as_str()
                .is_some_and(|content| content.contains("lockgate-coding-agent"))
        );
        Ok(())
    }

    async fn serve_inkling(listener: TcpListener) -> Result<Vec<serde_json::Value>> {
        let responses = [
            serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "reasoning_content": "I should inspect the manifest.",
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"Cargo.toml\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "The package is lockgate-coding-agent.",
                        "reasoning_content": "The manifest contains the package name."
                    },
                    "finish_reason": "stop"
                }]
            }),
        ];
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().await?;
            let request = read_request(&mut stream).await?;
            requests.push(serde_json::from_slice(&request)?);
            let body = response.to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(body.as_bytes()).await?;
        }
        Ok(requests)
    }

    async fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                bail!("HTTP client closed before sending a complete request");
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(head_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let body_start = head_end + 4;
            let head = std::str::from_utf8(&bytes[..head_end])?;
            let length = head
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>())
                    })
                })
                .transpose()?
                .context("request omitted Content-Length")?;
            if bytes.len() >= body_start + length {
                return Ok(bytes[body_start..body_start + length].to_vec());
            }
        }
    }
}
