//! Cross-layer tests for discovery, planning, authority, and fuel isolation.
//! Small synthesized components keep the library suite independent from the runnable demo.

use crate::{Application, ApplicationError, HostContext, Runtime, RuntimeBuildError, RuntimeError};
use crate::{
    binding::Binding,
    catalog::Catalog,
    runtime::{PluginStore, RuntimeComponent},
};
use wasmtime::component::{Accessor, Func, Instance};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

mod typed_bindings {
    crate::bindings! {
        path: "src/testdata/admission.wit",
        worlds: {
            RunnablePlugin: "runnable-plugin",
            SecondaryPlugin: "secondary-plugin",
        },
    }
}

mod client_bindings {
    crate::bindings! {
        path: "src/testdata/clients.wit",
        worlds: {
            QueryObserverPlugin: "query-observer-plugin",
            DatabaseConnectorPlugin: "database-connector-plugin",
        },
    }
}

#[allow(dead_code)]
async fn generated_clients_preserve_wit_signatures(
    runtime: &Runtime,
    observer: crate::Component<client_bindings::QueryObserverPlugin>,
    database: crate::Component<client_bindings::DatabaseConnectorPlugin>,
    query: client_bindings::__lockgate_world_0::exports::demo::clients::query_observer::Query,
    phase: client_bindings::__lockgate_world_0::exports::demo::clients::query_observer::Phase,
    interest: client_bindings::__lockgate_world_0::exports::demo::clients::query_observer::Interest,
) {
    let _ = runtime
        .component(observer)
        .observe(&query, Some(phase), interest)
        .await;
    let _ = runtime.component(database).execute("select 1", &[]).await;
}

impl typed_bindings::demo::admission::services::Host for HostContext<()> {
    fn log(&mut self, _message: String) {}
}

#[tokio::test]
async fn application_bindings_share_imported_host_interfaces() {
    assert_eq!(
        <typed_bindings::RunnablePlugin as Binding<()>>::HOST_IMPORTS,
        &["demo:admission/services@0.1.0"]
    );
    assert_eq!(
        <typed_bindings::SecondaryPlugin as Binding<()>>::HOST_IMPORTS,
        <typed_bindings::RunnablePlugin as Binding<()>>::HOST_IMPORTS,
    );

    let wit = r#"package demo:admission@0.1.0;

interface services { log: func(message: string); }
interface runnable { run: func() -> string; }
world matching { import services; export runnable; }"#;
    let mut app = Application::new(()).unwrap();
    let (runnable, secondary) = app
        .add::<(
            typed_bindings::RunnablePlugin,
            typed_bindings::SecondaryPlugin,
        )>(component_bytes_for(wit, "matching", "plugin"))
        .unwrap();
    assert_eq!(runnable.id(), secondary.id());
    assert_eq!(app.metadata(runnable).unwrap().id(), "plugin");
    assert_eq!(app.host_installer_count(), 1);
    app.allow_host_import(secondary, "demo:admission/services@0.1.0")
        .unwrap()
        .run()
        .await
        .unwrap();
}

fn component_bytes(wit: &str, world_name: &str) -> Vec<u8> {
    component_bytes_for(wit, world_name, world_name)
}

fn component_bytes_for(wit: &str, world_name: &str, plugin_id: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.packages[package].worlds[world_name];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    crate::catalog::with_test_plugin_metadata(bytes, plugin_id)
}

#[test]
fn typed_catalog_admission_uses_generated_world_exports() {
    let matching = r#"package demo:admission@0.1.0;

interface services { log: func(message: string); }
interface runnable { run: func() -> string; }
world matching { import services; export runnable; }"#;
    let mismatched = r#"package demo:admission@0.1.0;

interface runnable { run: func(input: string) -> string; }
world mismatched { export runnable; }"#;

    let mut catalog = Catalog::new().unwrap();
    let runnable = catalog
        .add::<(), typed_bindings::RunnablePlugin>(component_bytes_for(
            matching, "matching", "runnable",
        ))
        .unwrap();
    assert_eq!(
        catalog.entry(runnable.id()).unwrap().metadata.id(),
        "runnable"
    );

    let error = catalog
        .add::<(), typed_bindings::RunnablePlugin>(component_bytes_for(
            mismatched,
            "mismatched",
            "mismatched",
        ))
        .unwrap_err();
    assert!(matches!(error, ApplicationError::WorldMismatch { .. }));
}

#[test]
fn catalog_accepts_fixed_lists() {
    let wit = r#"package demo:fixed-list@0.1.0;

interface api { run: func(input: list<u32, 4>); }
world caller { import api; }"#;
    let mut catalog = Catalog::new().unwrap();
    catalog.add_untyped(component_bytes(wit, "caller")).unwrap();
}

#[tokio::test]
async fn plan_rejects_resource_handles_only_when_linked_across_stores() {
    let wit = r#"package demo:resources@0.1.0;

interface api {
  resource file;
  run: func(input: borrow<file>);
}
world caller { import api; }
world provider { export api; }"#;
    let mut app = Application::new(()).unwrap();
    let caller = app.add_untyped(component_bytes(wit, "caller")).unwrap();
    let provider = app.add_untyped(component_bytes(wit, "provider")).unwrap();
    assert!(matches!(
        app.link_ids(caller, provider).unwrap().run().await,
        Err(RuntimeBuildError::UnsupportedSiblingType { reason, .. })
            if reason.contains("resource")
    ));
}

#[tokio::test]
async fn plan_requires_an_explicit_link_even_when_the_provider_is_present() {
    let wit = r#"package demo:missing@0.1.0;
interface api { run: func(); }
world caller { import api; }
world provider { export api; }"#;
    let mut app = Application::new(()).unwrap();
    app.add_untyped(component_bytes(wit, "caller")).unwrap();
    app.add_untyped(component_bytes(wit, "provider")).unwrap();
    assert!(matches!(
        app.run().await,
        Err(RuntimeBuildError::MissingProvider { caller, interface })
            if caller == "caller" && interface == "demo:missing/api@0.1.0"
    ));
}

#[tokio::test]
async fn plan_rejects_ambiguous_providers() {
    let wit = r#"package demo:ambiguous@0.1.0;
interface api { run: func(); }
world caller { import api; }
world provider { export api; }"#;
    let mut app = Application::new(()).unwrap();
    let caller = app.add_untyped(component_bytes(wit, "caller")).unwrap();
    let first = app
        .add_untyped(component_bytes_for(wit, "provider", "first"))
        .unwrap();
    let second = app
        .add_untyped(component_bytes_for(wit, "provider", "second"))
        .unwrap();
    let app = app
        .link_ids(caller, first)
        .unwrap()
        .link_ids(caller, second)
        .unwrap();
    assert!(matches!(
        app.run().await,
        Err(RuntimeBuildError::AmbiguousProvider { .. })
    ));
}

#[tokio::test]
async fn plan_compares_complete_structural_types() {
    let caller_wit = structural_wit("s32", "safe", "polite", "choice", "caller", "import");
    let provider_wit = structural_wit("u32", "careful", "quiet", "selection", "provider", "export");
    let mut app = Application::new(()).unwrap();
    let caller = app
        .add_untyped(component_bytes(&caller_wit, "caller"))
        .unwrap();
    let provider = app
        .add_untyped(component_bytes(&provider_wit, "provider"))
        .unwrap();
    let error = match app.link_ids(caller, provider).unwrap().run().await {
        Ok(_) => panic!("mismatched structural types unexpectedly planned"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("type mismatch"));
    assert!(error.contains("variant"));
    assert!(error.contains("enum"));
    assert!(error.contains("flags"));
    assert!(error.contains("record"));
}

struct LoopBinding {
    function: Func,
}

impl Binding<()> for LoopBinding {
    const WORLD: &'static str = "loop";
    const EXPORTS: &'static [crate::binding::BindingExport] = &[];
    const HOST_IMPORTS: &'static [&'static str] = &[];

    type Client<'runtime> = ();

    fn bind(accessor: &Accessor<PluginStore<()>>, instance: &Instance) -> anyhow::Result<Self> {
        accessor.with(|mut access| {
            let interface = instance
                .get_export_index(&mut access, None, "demo:fuel/api@0.1.0")
                .ok_or_else(|| anyhow::anyhow!("interface is not exported"))?;
            let function = instance
                .get_export_index(&mut access, Some(&interface), "run")
                .ok_or_else(|| anyhow::anyhow!("function is not exported"))?;
            let function = instance
                .get_func(&mut access, function)
                .ok_or_else(|| anyhow::anyhow!("export is not a function"))?;
            Ok(Self { function })
        })
    }

    fn install_host_import(
        _interface: &str,
        _linker: &mut wasmtime::component::Linker<PluginStore<()>>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("loop binding has no host imports")
    }

    fn client<'runtime>(_component: RuntimeComponent<'runtime, (), Self>) -> Self::Client<'runtime>
    where
        (): 'runtime,
    {
    }
}

impl crate::binding::RoleSet<()> for LoopBinding {
    type Handles = crate::Component<Self>;

    #[allow(clippy::type_complexity)]
    fn for_each_role(
        visitor: &mut dyn FnMut(
            &'static str,
            &'static [crate::binding::BindingExport],
            &'static [&'static str],
            fn(&str, &mut wasmtime::component::Linker<PluginStore<()>>) -> anyhow::Result<()>,
        ),
    ) {
        visitor(
            <Self as Binding<()>>::WORLD,
            <Self as Binding<()>>::EXPORTS,
            <Self as Binding<()>>::HOST_IMPORTS,
            <Self as Binding<()>>::install_host_import,
        );
    }

    fn handles(component: crate::Component<Self>) -> Self::Handles {
        component
    }
}

#[tokio::test]
async fn fuel_trap_marks_only_the_looping_component_unhealthy() {
    let bytes = wat::parse_str(
        r#"(component
            (core module $m (func (export "loop") (loop $again (br $again))))
            (core instance $i (instantiate $m))
            (func $loop (canon lift (core func $i "loop")))
            (instance $api (export "run" (func $loop)))
            (export "demo:fuel/api@0.1.0" (instance $api)))"#,
    )
    .unwrap();
    let bytes = crate::catalog::with_test_plugin_metadata(bytes, "looping");
    let mut app = Application::new(()).unwrap();
    let looping = app.add::<LoopBinding>(bytes).unwrap();
    let runtime = app.run().await.unwrap();
    async fn call_loop(
        runtime: &Runtime,
        looping: crate::Component<LoopBinding>,
    ) -> Result<(), RuntimeError> {
        let component = RuntimeComponent::new(runtime, looping);
        component
            .invoke(|accessor, binding| {
                Box::pin(async move {
                    accessor.with(|mut access| binding.function.call(&mut access, &[], &mut []))?;
                    Ok(())
                })
            })
            .await
    }
    assert!(matches!(
        call_loop(&runtime, looping).await,
        Err(RuntimeError::Trapped { .. })
    ));
    assert!(matches!(
        call_loop(&runtime, looping).await,
        Err(RuntimeError::Unhealthy { .. })
    ));
}

fn structural_wit(
    payload: &str,
    enum_case: &str,
    flag: &str,
    record_field: &str,
    world: &str,
    direction: &str,
) -> String {
    format!(
        r#"package demo:structural@0.1.0;

interface api {{
  variant choice {{ text(string), number({payload}) }}
  enum mode {{ fast, {enum_case} }}
  flags options {{ loud, {flag} }}
  record request {{ {record_field}: choice, mode: mode, options: options }}
  run: func(input: request) -> request;
}}

world {world} {{ {direction} api; }}"#
    )
}
