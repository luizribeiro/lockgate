mod common;

use std::collections::HashMap;

use lockgate::PluginId;
use lockgate::{CallBudget, CallError, HostBuilder, PluginConfig, RuntimeLimits};

lockgate::host_bindings!({
    path: "tests/data/host_export_values",
    world: "fixture",
});

use mirror::HostExt as _;
use type_::HostExt as _;
use union_::HostExt as _;
use values::HostExt as _;

async fn host() -> (lockgate::Host<()>, lockgate::PluginHandle) {
    host_with_budgets(values::Budgets::default()).await
}

async fn host_with_budgets(
    budgets: values::Budgets,
) -> (lockgate::Host<()>, lockgate::PluginHandle) {
    let mut builder = HostBuilder::new(())
        .unwrap()
        .budgets::<values::Role>(budgets)
        .unwrap();
    let prepared = builder
        .prepare(
            PluginId::try_from("host-export-values").unwrap(),
            &common::HOST_EXPORT_VALUES_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
    (builder.finish(), plugin)
}

#[test]
fn generated_budget_structs_are_typed_and_default_every_function() {
    let defaults = CallBudget::default();
    assert_eq!(
        values::Budgets {
            round_trip: defaults,
            probe: defaults,
        },
        values::Budgets::default()
    );
    assert_eq!(mirror::Budgets::default().round_trip, defaults);
    assert_eq!(type_::Budgets::default().ping, defaults);
    assert_eq!(union_::Budgets::default().ping, defaults);
}

#[tokio::test]
async fn registered_budget_is_per_function_and_one_call_can_override_it() {
    let low = CallBudget {
        fuel: 1,
        deadline: common::INVOCATION_DEADLINE,
    };
    let (host, plugin) = host_with_budgets(values::Budgets {
        round_trip: low,
        ..values::Budgets::default()
    })
    .await;
    let values = host.values(&plugin).unwrap();
    let value = payload(
        values::Choice::Count(42),
        values::Color::Red,
        values::Permissions::READ,
    );

    assert!(matches!(
        values.round_trip(value.clone()).await,
        Err(CallError::OutOfBudget { fuel: 1 })
    ));
    assert_eq!(values.probe(true).await.unwrap(), Ok(42));
    assert_eq!(
        values
            .round_trip_with_budget(CallBudget::default(), value.clone())
            .await
            .unwrap(),
        value
    );
    host.shutdown().await;
}

#[test]
fn registered_budgets_reject_zero_fuel_and_deadlines() {
    for invalid in [
        CallBudget {
            fuel: 0,
            ..CallBudget::default()
        },
        CallBudget {
            deadline: std::time::Duration::ZERO,
            ..CallBudget::default()
        },
    ] {
        let result = HostBuilder::new(())
            .unwrap()
            .budgets::<values::Role>(values::Budgets {
                probe: invalid,
                ..values::Budgets::default()
            });
        assert!(result.is_err());
    }
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
    assert_eq!(values.round_trip(count.clone()).await.unwrap(), count,);

    let text = payload(
        values::Choice::Text("variant".into()),
        values::Color::Blue,
        values::Permissions::READ | values::Permissions::WRITE,
    );
    assert_eq!(mirror.round_trip(text.clone()).await.unwrap(), text,);

    let none = payload(
        values::Choice::None,
        values::Color::Red,
        values::Permissions::empty(),
    );
    assert_eq!(values.round_trip(none.clone()).await.unwrap(), none,);
    host.shutdown().await;
}

#[tokio::test]
async fn top_level_result_keeps_its_wit_data_channel() {
    let (host, plugin) = host().await;
    let values = host.values(&plugin).unwrap();

    assert!(matches!(values.probe(true).await, Ok(Ok(42)),));
    assert!(matches!(
        values.probe(false).await,
        Ok(Err(error)) if error == "rejected",
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn rust_keyword_interfaces_use_their_sanitized_modules() {
    let (host, plugin) = host().await;
    assert_eq!(host.type_(&plugin).unwrap().ping().await.unwrap(), 13,);
    assert_eq!(host.union_(&plugin).unwrap().ping().await.unwrap(), 17,);
    host.shutdown().await;
}
