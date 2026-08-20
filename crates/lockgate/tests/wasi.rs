mod common;

use common::exec::ExecEngine;
use common::{INVOCATION_FUEL, LIMITS, TestState};
use lockgate::{
    CallError, Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, Role, RoleInvocation,
    RuntimeLimits, Value,
};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRef};
use std::future::Future;
use std::process::Command;
use wasmtime::Result;
use wasmtime::component::Val;

const ENV_PLUGIN_ID: &str = "com.example.wasi-environment";
const CHILD_ENV_TEST: &str = "LOCKGATE_WASI_ENV_TEST_CHILD";

struct WasiRole;

struct WasiClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for WasiRole {
    const INTERFACE: &'static str = "test:wasi/guest";

    type Client<'a, S>
        = WasiClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        WasiClient(invocation)
    }
}

impl WasiClient<'_, ()> {
    async fn environment_count(&self) -> std::result::Result<u64, CallError> {
        let values = self
            .0
            .invoke(
                "environment-count",
                &[],
                InvocationCtx::bounded(INVOCATION_FUEL),
            )
            .await?;
        match values.as_slice() {
            [Value::U64(count)] => Ok(*count),
            _ => Err(CallError::shape("expected one u64 result")),
        }
    }

    async fn variable_absent(&self, name: &str) -> std::result::Result<bool, CallError> {
        self.invoke_bool("variable-absent", &[Value::String(name.to_owned())])
            .await
    }

    async fn variable_equals(
        &self,
        name: &str,
        value: &str,
    ) -> std::result::Result<bool, CallError> {
        self.invoke_bool(
            "variable-equals",
            &[
                Value::String(name.to_owned()),
                Value::String(value.to_owned()),
            ],
        )
        .await
    }

    async fn invoke_bool(
        &self,
        function: &str,
        arguments: &[Value],
    ) -> std::result::Result<bool, CallError> {
        let values = self
            .0
            .invoke(function, arguments, InvocationCtx::bounded(INVOCATION_FUEL))
            .await?;
        match values.as_slice() {
            [Value::Bool(value)] => Ok(*value),
            _ => Err(CallError::shape("expected one bool result")),
        }
    }
}

fn env_manifest(names: &[&str], optional: bool) -> NeedsManifest {
    let atom: AtomKey = "env.read".parse().unwrap();
    let entry = NeedEntry::scoped(
        atom,
        names
            .iter()
            .map(|name| ScopeRef::literal(*name).unwrap())
            .collect(),
    )
    .unwrap();
    let (required, optional) = if optional {
        (Vec::new(), vec![entry])
    } else {
        (vec![entry], Vec::new())
    };
    NeedsManifest::new(required, optional).unwrap()
}

async fn admitted_wasi(needs: &NeedsManifest) -> (Host<()>, PluginHandle) {
    let metadata = PluginMetadata::new(ENV_PLUGIN_ID, "WASI environment fixture", "1.0").unwrap();
    let component = common::policy_fixture(&common::WASI_FIXTURE, &metadata, needs);
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(ENV_PLUGIN_ID, &component, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(INVOCATION_FUEL),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

fn enter_controlled_environment(test_name: &str, variables: &[(&str, Option<&str>)]) -> bool {
    if std::env::var(CHILD_ENV_TEST).as_deref() == Ok(test_name) {
        return true;
    }

    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ENV_TEST, test_name);
    for (name, value) in variables {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "controlled child test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    false
}

fn run_async(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future);
}

#[test]
fn env_read_grant_exposes_only_the_named_host_variable() {
    const TEST_NAME: &str = "env_read_grant_exposes_only_the_named_host_variable";
    const GRANTED_NAME: &str = "LOCKGATE_TEST_GRANTED_8A55C94E";
    const GRANTED_VALUE: &str = "granted-value-1bf54ace";
    const UNGRANTED_NAME: &str = "LOCKGATE_TEST_UNGRANTED_8709E8AC";
    const UNGRANTED_VALUE: &str = "ungranted-value-935f7fbc";

    if !enter_controlled_environment(
        TEST_NAME,
        &[
            (GRANTED_NAME, Some(GRANTED_VALUE)),
            (UNGRANTED_NAME, Some(UNGRANTED_VALUE)),
        ],
    ) {
        return;
    }

    run_async(async {
        let (host, plugin) = admitted_wasi(&env_manifest(&[GRANTED_NAME], false)).await;
        let guest = host.client::<WasiRole>(&plugin).unwrap();

        assert!(
            guest
                .variable_equals(GRANTED_NAME, GRANTED_VALUE)
                .await
                .unwrap()
        );
        assert!(guest.variable_absent(UNGRANTED_NAME).await.unwrap());
        assert_eq!(guest.environment_count().await.unwrap(), 1);
    });
}

#[test]
fn optional_unset_env_read_grant_is_absent() {
    const TEST_NAME: &str = "optional_unset_env_read_grant_is_absent";
    const OPTIONAL_NAME: &str = "LOCKGATE_TEST_OPTIONAL_UNSET_5018A55C";

    if !enter_controlled_environment(TEST_NAME, &[(OPTIONAL_NAME, None)]) {
        return;
    }

    run_async(async {
        let (host, plugin) = admitted_wasi(&env_manifest(&[OPTIONAL_NAME], true)).await;
        let guest = host.client::<WasiRole>(&plugin).unwrap();

        assert!(guest.variable_absent(OPTIONAL_NAME).await.unwrap());
        assert_eq!(guest.environment_count().await.unwrap(), 0);
    });
}

#[test]
fn component_without_env_grants_has_an_empty_environment() {
    const TEST_NAME: &str = "component_without_env_grants_has_an_empty_environment";
    const HOST_ONLY_NAME: &str = "LOCKGATE_TEST_HOST_ONLY_4ACE8709";
    const HOST_ONLY_VALUE: &str = "host-only-value-e8ac935f";

    if !enter_controlled_environment(TEST_NAME, &[(HOST_ONLY_NAME, Some(HOST_ONLY_VALUE))]) {
        return;
    }

    run_async(async {
        let (host, plugin) = admitted_wasi(&NeedsManifest::empty()).await;
        let guest = host.client::<WasiRole>(&plugin).unwrap();

        assert!(guest.variable_absent(HOST_ONLY_NAME).await.unwrap());
        assert_eq!(guest.environment_count().await.unwrap(), 0);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn wasi_importing_guest_instantiates_and_runs() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::WASI_FIXTURE, |_| Ok(()))?;
    let clock = loaded
        .export("test:wasi/guest", "clock-seconds")
        .expect("clock export should resolve structurally");

    let results = loaded
        .invoke(clock, &[], TestState, LIMITS, INVOCATION_FUEL)
        .await?;

    assert!(matches!(results.as_slice(), [Val::U64(seconds)] if *seconds > 0));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn wasi_outbound_network_is_denied_at_runtime() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::WASI_FIXTURE, |_| Ok(()))?;
    let connect = loaded
        .export("test:wasi/guest", "network-denied")
        .expect("network denial export should resolve structurally");

    let results = loaded
        .invoke(connect, &[], TestState, LIMITS, INVOCATION_FUEL)
        .await?;

    assert!(matches!(results.as_slice(), [Val::Bool(true)]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn wasi_filesystem_access_is_denied_at_runtime() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::WASI_FIXTURE, |_| Ok(()))?;
    let open = loaded
        .export("test:wasi/guest", "filesystem-denied")
        .expect("filesystem denial export should resolve structurally");

    let results = loaded
        .invoke(open, &[], TestState, LIMITS, INVOCATION_FUEL)
        .await?;

    assert!(matches!(results.as_slice(), [Val::Bool(true)]));
    Ok(())
}
