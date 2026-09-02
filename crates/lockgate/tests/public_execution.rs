mod common;

use lockgate::PluginId;
use lockgate::{
    CallBudget, CallError, Host, HostBuilder, PluginConfig, PluginHandle, Role, RoleInvocation,
    RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const PLUGIN_ID: &str = "diagnostics";
const CALL_FUEL: u64 = 25_000_000;
const LOW_FUEL: u64 = 100_000;

fn budget(fuel: u64, deadline: std::time::Duration) -> CallBudget {
    CallBudget { fuel, deadline }
}

struct DiagnosticsRole;

struct DiagnosticsClient<'a, S: Send + Sync + 'static> {
    invocation: RoleInvocation<'a, S>,
}

impl Role for DiagnosticsRole {
    const INTERFACE: &'static str = "test:public/diagnostics";

    type Budgets = ();

    type Client<'a, S>
        = DiagnosticsClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        DiagnosticsClient { invocation }
    }
}

impl DiagnosticsClient<'_, ()> {
    async fn value(&self, budget: CallBudget) -> Result<u32, CallError> {
        self.call_u32("value", &[], budget).await
    }

    async fn pin(&self, budget: CallBudget) -> Result<u32, CallError> {
        self.call_u32("pin", &[], budget).await
    }

    async fn trap(&self, budget: CallBudget) -> Result<(), CallError> {
        let results = self
            .invocation
            .invoke_with_budget("trap", &[], (), budget)
            .await?;
        if results.is_empty() {
            Ok(())
        } else {
            Err(CallError::shape("trap returned unexpected results"))
        }
    }

    async fn work(&self, budget: CallBudget, iterations: u32) -> Result<u32, CallError> {
        self.call_u32("work", &[Value::U32(iterations)], budget)
            .await
    }

    async fn call_u32(
        &self,
        function: &str,
        arguments: &[Value],
        budget: CallBudget,
    ) -> Result<u32, CallError> {
        let results = self
            .invocation
            .invoke_with_budget(function, arguments, (), budget)
            .await?;
        match results.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape(format!(
                "{function} returned a non-u32 result"
            ))),
        }
    }
}

async fn admitted_fixture() -> (Host<()>, PluginHandle) {
    let mut builder = HostBuilder::new(()).unwrap();
    let plugin = admit(
        &mut builder,
        PluginId::from(PLUGIN_ID),
        &common::PUBLIC_FIXTURE,
    )
    .await;
    (builder.finish(), plugin)
}

async fn admit(
    builder: &mut HostBuilder<()>,
    plugin_id: PluginId,
    component: &[u8],
) -> PluginHandle {
    let metadata = PluginMetadata::new(plugin_id.as_str(), "Enumeration fixture", "1.0").unwrap();
    let bytes = common::sectioned_fixture(component, &metadata);
    let prepared = builder
        .prepare(plugin_id, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap()
}

fn unrelated_component() -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve
        .push_str(
            "unrelated.wit",
            "package test:unrelated; interface other { ping: func(); } world fixture { export other; }",
        )
        .unwrap();
    let world = resolve.select_world(&[package], None).unwrap();
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

#[tokio::test]
async fn role_clients_skip_non_exporters_and_yield_invokable_clients() {
    let mut builder = HostBuilder::new(()).unwrap();
    let skipped = admit(
        &mut builder,
        PluginId::from("unrelated"),
        &unrelated_component(),
    )
    .await;
    let implementing = admit(
        &mut builder,
        PluginId::from("diagnostics"),
        &common::PUBLIC_FIXTURE,
    )
    .await;
    let host = builder.finish();

    let mut clients = host.clients::<DiagnosticsRole>();
    let (plugin, diagnostics) = clients.next().expect("the implementing plugin was skipped");
    assert_ne!(plugin, &skipped);
    assert_eq!(plugin, &implementing);
    assert!(clients.next().is_none());
    assert_eq!(
        diagnostics
            .value(budget(CALL_FUEL, common::INVOCATION_DEADLINE))
            .await
            .unwrap(),
        42
    );
    drop(clients);
    host.shutdown().await;
}

#[tokio::test]
async fn role_clients_are_empty_when_no_plugin_exports_the_role() {
    let mut builder = HostBuilder::new(()).unwrap();
    admit(
        &mut builder,
        PluginId::from("unrelated"),
        &unrelated_component(),
    )
    .await;
    let host = builder.finish();

    assert_eq!(host.clients::<DiagnosticsRole>().count(), 0);
    host.shutdown().await;
}

#[tokio::test]
async fn role_clients_follow_admission_order() {
    let mut builder = HostBuilder::new(()).unwrap();
    let first = admit(
        &mut builder,
        PluginId::from("first"),
        &common::PUBLIC_FIXTURE,
    )
    .await;
    let second = admit(
        &mut builder,
        PluginId::from("second"),
        &common::PUBLIC_FIXTURE,
    )
    .await;
    let host = builder.finish();

    let plugins = host
        .clients::<DiagnosticsRole>()
        .map(|(plugin, _)| plugin)
        .collect::<Vec<_>>();
    assert_eq!(plugins, [&first, &second]);
    host.shutdown().await;
}

#[tokio::test]
async fn concurrently_submitted_calls_complete_without_a_busy_error() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    // Genuine in-flight overlap is proven by the internal barrier test on the
    // same execution path this public client delegates to. This import-free
    // fixture has no await point; the public barrier version arrives with host imports.
    let (first, second) = tokio::join!(
        diagnostics.work(budget(CALL_FUEL, common::INVOCATION_DEADLINE), 1_000),
        diagnostics.work(budget(CALL_FUEL, common::INVOCATION_DEADLINE), 1_000),
    );

    assert_eq!(first.unwrap(), second.unwrap());
    host.shutdown().await;
}

#[tokio::test]
async fn a_guest_trap_does_not_poison_the_next_call() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    let error = diagnostics
        .trap(budget(CALL_FUEL, common::INVOCATION_DEADLINE))
        .await
        .unwrap_err();
    assert!(matches!(error, CallError::Trap { .. }));
    assert_eq!(
        diagnostics
            .value(budget(CALL_FUEL, common::INVOCATION_DEADLINE))
            .await
            .unwrap(),
        42
    );
    host.shutdown().await;
}

#[tokio::test]
async fn every_call_gets_fresh_guest_globals() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    for _ in 0..2 {
        assert_eq!(
            diagnostics
                .pin(budget(CALL_FUEL, common::INVOCATION_DEADLINE))
                .await
                .unwrap(),
            1
        );
    }
    host.shutdown().await;
}

#[tokio::test]
async fn equal_fuel_exhausts_at_the_same_iteration_boundary() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    let first = first_exhausted_iteration(&diagnostics, LOW_FUEL).await;
    let second = first_exhausted_iteration(&diagnostics, LOW_FUEL).await;
    assert!(first > 1, "the guest should make deterministic progress");
    assert_eq!(first, second);

    let error = diagnostics
        .work(budget(LOW_FUEL, common::INVOCATION_DEADLINE), first)
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        CallError::OutOfBudget { fuel } if *fuel == LOW_FUEL
    ));
    assert!(error.to_string().contains(&LOW_FUEL.to_string()));
    host.shutdown().await;
}

async fn first_exhausted_iteration(diagnostics: &DiagnosticsClient<'_, ()>, fuel: u64) -> u32 {
    let mut completes = 0;
    let mut exhausts = 10_000;
    let upper_error = diagnostics
        .work(budget(fuel, common::INVOCATION_DEADLINE), exhausts)
        .await
        .expect_err("the boundary-search upper limit should exhaust its fuel");
    assert!(matches!(
        upper_error,
        CallError::OutOfBudget { fuel: exhausted } if exhausted == fuel
    ));

    while completes + 1 < exhausts {
        let candidate = completes + (exhausts - completes) / 2;
        match diagnostics
            .work(budget(fuel, common::INVOCATION_DEADLINE), candidate)
            .await
        {
            Ok(_) => completes = candidate,
            Err(CallError::OutOfBudget { fuel: exhausted }) => {
                assert_eq!(exhausted, fuel);
                exhausts = candidate;
            }
            Err(error) => panic!("boundary search failed unexpectedly: {error}"),
        }
    }
    exhausts
}
