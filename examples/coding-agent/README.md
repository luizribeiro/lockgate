# Coding agent example

This example is a small terminal coding agent connected to the same Inkling model server as the
local Pi configuration. Its provider and tools are isolated WebAssembly components, while the host
owns the conversation loop, TUI, and capabilities:

- `provider-plugin` owns a typed WIT `conversation` resource and exchanges tool specifications,
  prompts, tool results, tool calls, and completions.
- `tool-plugin` describes one JSON-schema tool and executes it.
- `http-client` gives a granted provider policy-limited HTTP without knowing its wire protocol.
- `provider-settings` gives a provider opaque, namespaced key/value configuration.
- `workspace` lets only explicitly granted tools inspect the selected workspace.

The Inkling component—not the host—owns defaults matching `~/.pi/agent/models.json`:

| Setting | Default |
| --- | --- |
| Base URL | `http://127.0.0.1:8080/v1` |
| Model | `Inkling-Small-mxfp4` |
| Maximum output | 2,048 tokens |

The provider's conversation resource privately preserves Inkling's `reasoning_content` so tool-call
turns can be replayed in the format the model expects. The host never receives that field or
reconstructs provider wire messages. JSON remains only where it is part of an external protocol:
OpenAI-compatible HTTP requests, JSON Schema, and dynamic tool arguments.

## Run it

Start Inkling as usual:

```sh
inklingrs serve models/Inkling-Small-mxfp4
```

The component build uses the same WASIp2 toolchain as `examples/demo` and requires `WASI_SYSROOT`.
From the repository root:

```sh
cargo run -p lockgate-coding-agent -- .
```

Ask normal questions such as “What does this repository do?” The model can call `list_files` and
`read_file`. Press Enter to submit a prompt and Esc or Ctrl-C to quit.

Provider settings are populated generically from `CODING_AGENT_PROVIDER_*`; the Inkling component
chooses which keys it understands:

```sh
CODING_AGENT_PROVIDER_BASE_URL=http://127.0.0.1:8080/v1 \
CODING_AGENT_PROVIDER_MODEL=Inkling-Small-mxfp4 \
CODING_AGENT_PROVIDER_MAX_TOKENS=2048 \
cargo run -p lockgate-coding-agent -- .
```

HTTP is limited to the exact origin `http://127.0.0.1:8080` by default. A comma-separated
`CODING_AGENT_HTTP_ORIGINS` allowlist can grant other explicit origins to whichever provider
component is admitted.

## Add a provider

Create a component implementing the `provider` world in `wit/worlds.wit`, embed Lockgate plugin
metadata, and admit it as `bindings::ProviderPlugin` in `src/main.rs`. The included Inkling provider
is a small OpenAI adapter: it owns the Inkling endpoint defaults, model configuration, wire format,
response interpretation, and conversation replay state. It uses the host's generic `http-client`
and `provider-settings` capabilities; the main crate contains no Inkling protocol or configuration
logic. The host retains only provider-neutral transcript entries for display and tool dispatch.

A provider with a different transport can import another application-owned host interface while
keeping the same typed provider export.

Set `LOCKGATE_PROVIDER_COMPONENT` to a compatible component path to replace the bundled provider
without changing the host:

```sh
LOCKGATE_PROVIDER_COMPONENT=/path/to/provider.wasm \
cargo run -p lockgate-coding-agent -- .
```

## Add a tool

Create a component implementing the `tool` world, add it to `components/Cargo.toml` and `build.rs`,
then admit and register it in `src/main.rs`. Import only the host interfaces it needs and grant each
one explicitly. `list-files` and `read-file` are separate components to make that boundary visible.

The host assigns a 25-million-unit Wasmtime fuel budget to initialization and each plugin call.
Lockgate restores that bounded budget at every call boundary.
