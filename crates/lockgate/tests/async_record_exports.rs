mod common;

use lockgate::{HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

lockgate::host_bindings!({
    path: "tests/fixtures/async-record-guest/wit",
    world: "fixture",
});

use guest::HostExt as _;

#[tokio::test]
async fn async_export_with_params_and_rich_result_round_trips() {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "async-record",
            &common::ASYNC_RECORD_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
        )
        .await
        .unwrap();
    let host = builder.finish();
    let guest = host.guest(&plugin).unwrap();

    let reply = guest
        .run(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            guest::Request {
                prompt: "hello".into(),
                limit: 7,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.model, "fixture-7");
    assert!(matches!(
        reply.pieces.as_slice(),
        [guest::Piece::Text(text), guest::Piece::Stop] if text == "hello"
    ));

    let error = guest
        .run(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            guest::Request {
                prompt: String::new(),
                limit: 0,
            },
        )
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error, "prompt must not be empty");
}
