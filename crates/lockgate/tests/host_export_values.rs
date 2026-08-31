mod common;

use std::collections::HashMap;

use lockgate::{HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

lockgate::host_bindings!({
    path: "tests/data/host_export_values",
    world: "fixture",
});

use mirror::HostExt as _;
use type_::HostExt as _;
use union_::HostExt as _;
use values::HostExt as _;

async fn host() -> (lockgate::Host<()>, lockgate::PluginHandle) {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "host-export-values",
            &common::HOST_EXPORT_VALUES_FIXTURE,
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
    (builder.finish(), plugin)
}

fn payload(
    choice: values::Choice,
    color: values::Color,
    permissions: values::Permissions,
) -> values::Payload {
    values::Payload {
        pair: (7, "tuple".into()),
        fixed: [3, 1, 4],
        lookup: HashMap::from([("one".into(), 1), ("two".into(), 2)]),
        choice,
        color,
        permissions,
    }
}

#[tokio::test]
async fn exotic_values_and_shared_records_round_trip_through_both_clients() {
    let (host, plugin) = host().await;
    let values = host.values(&plugin).unwrap();
    let mirror = host.mirror(&plugin).unwrap();

    let count = payload(
        values::Choice::Count(42),
        values::Color::Red,
        values::Permissions::READ,
    );
    assert_eq!(
        values
            .round_trip(
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
                count.clone()
            )
            .await
            .unwrap(),
        count,
    );

    let text = payload(
        values::Choice::Text("variant".into()),
        values::Color::Blue,
        values::Permissions::READ | values::Permissions::WRITE,
    );
    assert_eq!(
        mirror
            .round_trip(
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
                text.clone()
            )
            .await
            .unwrap(),
        text,
    );

    let none = payload(
        values::Choice::None,
        values::Color::Red,
        values::Permissions::empty(),
    );
    assert_eq!(
        values
            .round_trip(
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
                none.clone()
            )
            .await
            .unwrap(),
        none,
    );
    host.shutdown().await;
}

#[tokio::test]
async fn top_level_result_keeps_its_wit_data_channel() {
    let (host, plugin) = host().await;
    let values = host.values(&plugin).unwrap();

    assert!(matches!(
        values
            .probe(
                InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
                true
            )
            .await,
        Ok(Ok(42)),
    ));
    assert!(matches!(
        values
            .probe(InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE), false)
            .await,
        Ok(Err(error)) if error == "rejected",
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn rust_keyword_interfaces_use_their_sanitized_modules() {
    let (host, plugin) = host().await;
    assert_eq!(
        host.type_(&plugin)
            .unwrap()
            .ping(InvocationCtx::bounded(
                common::INVOCATION_FUEL,
                common::INVOCATION_DEADLINE
            ))
            .await
            .unwrap(),
        13,
    );
    assert_eq!(
        host.union_(&plugin)
            .unwrap()
            .ping(InvocationCtx::bounded(
                common::INVOCATION_FUEL,
                common::INVOCATION_DEADLINE
            ))
            .await
            .unwrap(),
        17,
    );
    host.shutdown().await;
}
