mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use lockgate::{
    Acceptance, BudgetClass, CallError, HostBuilder, HostCtx, InvocationCtx, PluginConfig, Role,
    RoleInvocation, RuntimeLimits, Value,
};
use tokio::sync::Barrier;

const REGRESSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct CallData {
    label: String,
    startup: u32,
}

#[derive(Clone)]
struct Imports {
    barrier: Arc<Barrier>,
    entries: Arc<AtomicUsize>,
    startups: Arc<std::sync::Mutex<Vec<u32>>>,
}

lockgate::host_bindings!({
    path: "tests/data/host_bindings",
    world: "fixture",
    imports: Imports,
    data: CallData,
});

impl application::Host for Imports {
    async fn read_data(&mut self, cx: HostCtx<'_, CallData>) -> String {
        self.barrier.wait().await;
        cx.data().label.clone()
    }

    async fn caller(&mut self, cx: HostCtx<'_, CallData>) -> String {
        cx.plugin().id().to_owned()
    }

    async fn transform(
        &mut self,
        _cx: HostCtx<'_, CallData>,
        request: application::Request,
    ) -> Result<application::Request, String> {
        if request.fail {
            Err("guest-visible".into())
        } else {
            Ok(request)
        }
    }

    async fn first(&mut self, _cx: HostCtx<'_, CallData>) {
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.barrier.wait().await;
    }

    async fn second(&mut self, _cx: HostCtx<'_, CallData>) {
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.barrier.wait().await;
    }

    async fn startup(&mut self, cx: HostCtx<'_, CallData>) -> u32 {
        self.startups.lock().unwrap().push(cx.data().startup);
        cx.data().startup
    }
}

struct GuestRole;

struct GuestClient<'a, S: Send + Sync + 'static> {
    invocation: RoleInvocation<'a, S>,
}

impl Role for GuestRole {
    const INTERFACE: &'static str = "test:host-bindings/guest";

    type Client<'a, S>
        = GuestClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        GuestClient { invocation }
    }
}

impl<S: Send + Sync + 'static> GuestClient<'_, S> {
    async fn string(&self, function: &str, ctx: InvocationCtx<S>) -> Result<String, CallError> {
        let values = self.invocation.invoke(function, &[], ctx).await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }

    async fn overlap(&self, ctx: InvocationCtx<S>) -> Result<u32, CallError> {
        let values = self.invocation.invoke("overlap", &[], ctx).await?;
        match values.as_slice() {
            [Value::U32(value)] => Ok(*value),
            _ => Err(CallError::shape("expected one u32 result")),
        }
    }

    async fn rich(
        &self,
        value: u32,
        fail: bool,
        ctx: InvocationCtx<S>,
    ) -> Result<String, CallError> {
        let values = self
            .invocation
            .invoke("rich", &[Value::U32(value), Value::Bool(fail)], ctx)
            .await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }
}

async fn host() -> (
    lockgate::Host<CallData>,
    lockgate::PluginHandle,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<u32>>>,
) {
    let entries = Arc::new(AtomicUsize::new(0));
    let startups = Arc::new(std::sync::Mutex::new(Vec::new()));
    let imports = Imports {
        barrier: Arc::new(Barrier::new(2)),
        entries: Arc::clone(&entries),
        startups: Arc::clone(&startups),
    };
    let mut builder = HostBuilder::<CallData>::new(imports).unwrap();
    let prepared = builder
        .prepare(
            "host-caller",
            &common::HOST_BINDINGS_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::new(
                CallData {
                    label: "startup".into(),
                    startup: 17,
                },
                BudgetClass::Bounded { fuel: 1_000_000 },
            ),
        )
        .await
        .unwrap();
    (builder.finish(), plugin, entries, startups)
}

fn context(label: &str) -> InvocationCtx<CallData> {
    InvocationCtx::new(
        CallData {
            label: label.into(),
            startup: 0,
        },
        BudgetClass::Bounded { fuel: 1_000_000 },
    )
}

#[tokio::test]
async fn concurrent_invocations_observe_their_own_data() {
    let (host, plugin, _, _) = host().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    let (left, right) = tokio::time::timeout(REGRESSION_TIMEOUT, async {
        tokio::join!(
            guest.string("data", context("left")),
            guest.string("data", context("right"))
        )
    })
    .await
    .expect("invocations did not overlap at the host-import barrier");
    assert_eq!(left.unwrap(), "left");
    assert_eq!(right.unwrap(), "right");
}

#[tokio::test]
async fn host_context_identifies_the_calling_plugin() {
    let (host, plugin, _, _) = host().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    assert_eq!(
        guest.string("caller", context("call")).await.unwrap(),
        "host-caller"
    );
}

#[tokio::test]
async fn generated_host_imports_preserve_overlapping_calls() {
    let (host, plugin, entries, _) = host().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    let result = tokio::time::timeout(REGRESSION_TIMEOUT, guest.overlap(context("call")))
        .await
        .expect("serialized generated host imports deadlocked at the barrier");
    assert_eq!(result.unwrap(), 2);
    assert_eq!(entries.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn smoke_imports_observe_startup_data() {
    let startups = Arc::new(std::sync::Mutex::new(Vec::new()));
    let imports = Imports {
        barrier: Arc::new(Barrier::new(2)),
        entries: Arc::new(AtomicUsize::new(0)),
        startups: Arc::clone(&startups),
    };
    let component = wat::parse_str(
        r#"(component
            (type $application (instance
                (export "startup" (func (result u32)))
            ))
            (import "test:host-bindings/application" (instance $application (type $application)))
            (alias export $application "startup" (func $startup))
            (core func $startup-lowered (canon lower (func $startup)))
            (core module $module
                (import "" "startup" (func $startup (result i32)))
                (func $start
                    call $startup
                    drop)
                (start $start)
            )
            (core instance $instance
                (instantiate $module
                    (with "" (instance
                        (export "startup" (func $startup-lowered))
                    ))
                )
            )
        )"#,
    )
    .unwrap();
    let metadata =
        lockgate_schema::PluginMetadata::new("smoke-caller", "Smoke caller", "1.0").unwrap();
    let component = common::sectioned_fixture(&component, &metadata);
    let mut builder = HostBuilder::<CallData>::new(imports).unwrap();
    let prepared = builder
        .prepare("smoke-caller", &component, PluginConfig::default())
        .await
        .unwrap();
    builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            InvocationCtx::new(
                CallData {
                    label: "startup".into(),
                    startup: 17,
                },
                BudgetClass::Bounded { fuel: 1_000_000 },
            ),
        )
        .await
        .unwrap();
    assert_eq!(*startups.lock().unwrap(), [17]);
}

#[tokio::test]
async fn rich_import_types_keep_wit_results_in_the_guest_data_channel() {
    let (host, plugin, _, _) = host().await;
    let guest = host.client::<GuestRole>(&plugin).unwrap();
    assert_eq!(guest.rich(7, false, context("call")).await.unwrap(), "ok:7");
    assert_eq!(
        guest.rich(7, true, context("call")).await.unwrap(),
        "guest-visible"
    );
}

#[tokio::test]
async fn each_admitted_plugin_is_named_by_its_own_host_context() {
    let imports = Imports {
        barrier: Arc::new(Barrier::new(2)),
        entries: Arc::new(AtomicUsize::new(0)),
        startups: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let mut builder = HostBuilder::<CallData>::new(imports).unwrap();
    let first = builder
        .prepare(
            "host-caller",
            &common::HOST_BINDINGS_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let first = builder
        .admit(
            first,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            context("startup"),
        )
        .await
        .unwrap();

    let metadata =
        lockgate_schema::PluginMetadata::new("other-caller", "Other caller", "1.0").unwrap();
    let second_bytes = common::sectioned_fixture(&common::HOST_BINDINGS_FIXTURE, &metadata);
    let second = builder
        .prepare("other-caller", &second_bytes, PluginConfig::default())
        .await
        .unwrap();
    let second = builder
        .admit(
            second,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            context("startup"),
        )
        .await
        .unwrap();

    let host = builder.finish();
    let first_guest = host.client::<GuestRole>(&first).unwrap();
    let second_guest = host.client::<GuestRole>(&second).unwrap();
    assert_eq!(
        first_guest.string("caller", context("call")).await.unwrap(),
        "host-caller"
    );
    assert_eq!(
        second_guest
            .string("caller", context("call"))
            .await
            .unwrap(),
        "other-caller"
    );
}
