mod common;

use lockgate::{
    Acceptance, CallError, Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, Role,
    RoleInvocation, RuntimeLimits, Value,
};
use lockgate_schema::PluginMetadata;

const PLUGIN_ID: &str = "diagnostics";
const CALL_FUEL: u64 = 25_000_000;
const LOW_FUEL: u64 = 100_000;

struct DiagnosticsRole;

struct DiagnosticsClient<'a, S: Send + Sync + 'static> {
    invocation: RoleInvocation<'a, S>,
}

impl Role for DiagnosticsRole {
    const INTERFACE: &'static str = "test:public/diagnostics";

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

impl<S: Send + Sync + 'static> DiagnosticsClient<'_, S> {
    async fn value(&self, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        self.call_u32("value", &[], ctx).await
    }

    async fn pin(&self, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        self.call_u32("pin", &[], ctx).await
    }

    async fn trap(&self, ctx: InvocationCtx<S>) -> Result<(), CallError> {
        let results = self.invocation.invoke("trap", &[], ctx).await?;
        if results.is_empty() {
            Ok(())
        } else {
            Err(CallError::shape("trap returned unexpected results"))
        }
    }

    async fn work(&self, ctx: InvocationCtx<S>, iterations: u32) -> Result<u32, CallError> {
        self.call_u32("work", &[Value::U32(iterations)], ctx).await
    }

    async fn call_u32(
        &self,
        function: &str,
        arguments: &[Value],
        ctx: InvocationCtx<S>,
    ) -> Result<u32, CallError> {
        let results = self.invocation.invoke(function, arguments, ctx).await?;
        match results.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape(format!(
                "{function} returned a non-u32 result"
            ))),
        }
    }
}

async fn admitted_fixture() -> (Host<()>, PluginHandle) {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Diagnostics", "1.0").unwrap();
    let bytes = common::sectioned_fixture(&common::PUBLIC_FIXTURE, &metadata);
    let mut builder = HostBuilder::<()>::new(()).unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

#[tokio::test]
async fn concurrently_submitted_calls_complete_without_a_busy_error() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    // Genuine in-flight overlap is proven by the internal barrier test on the
    // same execution path this public client delegates to. This import-free
    // fixture has no await point; the public barrier version arrives with host imports.
    let (first, second) = tokio::join!(
        diagnostics.work(InvocationCtx::bounded(CALL_FUEL), 1_000),
        diagnostics.work(InvocationCtx::bounded(CALL_FUEL), 1_000),
    );

    assert_eq!(first.unwrap(), second.unwrap());
}

#[tokio::test]
async fn a_guest_trap_does_not_poison_the_next_call() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    let error = diagnostics
        .trap(InvocationCtx::bounded(CALL_FUEL))
        .await
        .unwrap_err();
    assert!(matches!(error, CallError::Trap { .. }));
    assert_eq!(
        diagnostics
            .value(InvocationCtx::bounded(CALL_FUEL))
            .await
            .unwrap(),
        42
    );
}

#[tokio::test]
async fn every_call_gets_fresh_guest_globals() {
    let (host, plugin) = admitted_fixture().await;
    let diagnostics = host.client::<DiagnosticsRole>(&plugin).unwrap();

    for _ in 0..2 {
        assert_eq!(
            diagnostics
                .pin(InvocationCtx::bounded(CALL_FUEL))
                .await
                .unwrap(),
            1
        );
    }
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
        .work(InvocationCtx::bounded(LOW_FUEL), first)
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        CallError::OutOfBudget { fuel } if *fuel == LOW_FUEL
    ));
    assert!(error.to_string().contains(&LOW_FUEL.to_string()));
}

async fn first_exhausted_iteration(diagnostics: &DiagnosticsClient<'_, ()>, fuel: u64) -> u32 {
    let mut completes = 0;
    let mut exhausts = 10_000;
    let upper_error = diagnostics
        .work(InvocationCtx::bounded(fuel), exhausts)
        .await
        .expect_err("the boundary-search upper limit should exhaust its fuel");
    assert!(matches!(
        upper_error,
        CallError::OutOfBudget { fuel: exhausted } if exhausted == fuel
    ));

    while completes + 1 < exhausts {
        let candidate = completes + (exhausts - completes) / 2;
        match diagnostics
            .work(InvocationCtx::bounded(fuel), candidate)
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
