mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use lockgate::{
    Acceptance, BudgetClass, HostBuilder, HostCtx, InvocationCtx, PluginConfig, RoleError,
    RuntimeLimits,
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

use guest::HostExt;

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
    let guest = host.guest(&plugin).unwrap();
    let (left, right) = tokio::time::timeout(REGRESSION_TIMEOUT, async {
        tokio::join!(guest.data(context("left")), guest.data(context("right")))
    })
    .await
    .expect("invocations did not overlap at the host-import barrier");
    assert_eq!(left.unwrap(), "left");
    assert_eq!(right.unwrap(), "right");
}

#[tokio::test]
async fn host_context_identifies_the_calling_plugin() {
    let (host, plugin, _, _) = host().await;
    let guest = host.guest(&plugin).unwrap();
    assert_eq!(guest.caller(context("call")).await.unwrap(), "host-caller");
}

#[tokio::test]
async fn generated_host_imports_preserve_overlapping_calls() {
    let (host, plugin, entries, _) = host().await;
    let guest = host.guest(&plugin).unwrap();
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
    let guest = host.guest(&plugin).unwrap();
    assert_eq!(guest.rich(context("call"), 7, false).await.unwrap(), "ok:7");
    assert_eq!(
        guest.rich(context("call"), 7, true).await.unwrap(),
        "guest-visible"
    );
}

fn payload(outcome: Result<u64, String>) -> guest::Payload {
    guest::Payload {
        flag: true,
        unsigned_8: 8,
        unsigned_16: 16,
        unsigned_32: 32,
        unsigned_64: 64,
        signed_8: -8,
        signed_16: -16,
        signed_32: -32,
        signed_64: -64,
        float_32: 3.25,
        float_64: -6.5,
        letter: 'λ',
        text: "typed".into(),
        items: vec![2, 4, 8],
        maybe: Some("optional".into()),
        outcome,
    }
}

fn assert_payload(value: guest::Payload, outcome: Result<u64, String>) {
    assert!(value.flag);
    assert_eq!(value.unsigned_8, 8);
    assert_eq!(value.unsigned_16, 16);
    assert_eq!(value.unsigned_32, 32);
    assert_eq!(value.unsigned_64, 64);
    assert_eq!(value.signed_8, -8);
    assert_eq!(value.signed_16, -16);
    assert_eq!(value.signed_32, -32);
    assert_eq!(value.signed_64, -64);
    assert_eq!(value.float_32, 3.25);
    assert_eq!(value.float_64, -6.5);
    assert_eq!(value.letter, 'λ');
    assert_eq!(value.text, "typed");
    assert_eq!(value.items, [2, 4, 8]);
    assert_eq!(value.maybe.as_deref(), Some("optional"));
    assert_eq!(value.outcome, outcome);
}

#[tokio::test]
async fn generated_clients_round_trip_value_shapes_through_both_casts() {
    let (host, plugin, _, _) = host().await;

    let extension = host.guest(&plugin).unwrap();
    let via_extension = extension
        .round_trip(context("call"), payload(Ok(99)))
        .await
        .unwrap();
    let generic = host.client::<guest::Role>(&plugin).unwrap();
    let via_generic = generic
        .round_trip(context("call"), payload(Err("guest-data".into())))
        .await
        .unwrap();

    assert_payload(via_extension, Ok(99));
    assert_payload(via_generic, Err("guest-data".into()));
}

#[tokio::test]
async fn generated_role_fails_at_the_cast_when_not_exported() {
    let imports = Imports {
        barrier: Arc::new(Barrier::new(2)),
        entries: Arc::new(AtomicUsize::new(0)),
        startups: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let mut builder = HostBuilder::<CallData>::new(imports).unwrap();
    let prepared = builder
        .prepare("greeter", &common::PUBLIC_FIXTURE, PluginConfig::default())
        .await
        .unwrap();
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            context("startup"),
        )
        .await
        .unwrap();
    let host = builder.finish();

    let error = match host.client::<guest::Role>(&plugin) {
        Ok(_) => panic!("generated role cast unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(
        error,
        RoleError::RoleNotExported {
            interface: "test:host-bindings/guest",
        }
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
    let first_guest = host.guest(&first).unwrap();
    let second_guest = host.guest(&second).unwrap();
    assert_eq!(
        first_guest.caller(context("call")).await.unwrap(),
        "host-caller"
    );
    assert_eq!(
        second_guest.caller(context("call")).await.unwrap(),
        "other-caller"
    );
}
