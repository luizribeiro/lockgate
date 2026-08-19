use std::process::Command;
use std::{path::PathBuf, sync::Arc};

use lockgate::{
    CapabilityContract, Permission, PluginSubject, ResolveCtx, ResolveScopedResource,
    ResolveScopedResourceHandle, Resource, ResourceLookupError, Scope, ScopeRepr, ScopedPermission,
    ScopedResource,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ScopeRepr)]
enum FacadeScope {
    Project,
}

impl Scope for FacadeScope {}

struct FacadeResource;

impl ScopedResource<FacadeScope> for FacadeResource {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<FacadeScope> {
        vec![FacadeScope::Project]
    }
}

struct FacadeResolver;

impl ResolveScopedResource<FacadeScope, str> for FacadeResolver {
    type Resource = FacadeResource;
    type Error = ();

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        _argument: &'a str,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(FacadeResource)
    }
}

enum ResourceMarker {}

struct ResourceResolver;

impl ResolveScopedResourceHandle<FacadeScope, ResourceMarker, ()> for ResourceResolver {
    type Representation = FacadeResource;
    type Resource = Arc<FacadeResource>;
    type Error = ResourceLookupError;

    async fn resolve_scoped_resource_handle<'a>(
        &'a self,
        _context: &'a ResolveCtx<'_, ()>,
        representation: Arc<Self::Representation>,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(representation)
    }
}

#[test]
fn host_facade_exposes_permission_contract_types() {
    fn accepts_contract<T: CapabilityContract>() {}
    fn accepts_unscoped(_: Option<Permission>) {}
    fn accepts_scoped(_: Option<ScopedPermission<FacadeScope>>) {}
    fn accepts_resource<T: ScopedResource<FacadeScope>>() {}
    fn accepts_resolver<T: ResolveScopedResource<FacadeScope, str>>() {}
    fn accepts_resource_resolver<
        T: ResolveScopedResourceHandle<FacadeScope, ResourceMarker, ()>,
    >() {
    }

    accepts_unscoped(None);
    accepts_scoped(None);
    accepts_contract::<FixtureContract>();
    accepts_resource::<FacadeResource>();
    accepts_resolver::<FacadeResolver>();
    accepts_resource_resolver::<ResourceResolver>();
    let _ = core::mem::size_of::<ResolveCtx<'_, ()>>();
    let _ = core::mem::size_of::<Resource<ResourceMarker>>();
    let _ = core::mem::size_of::<ResourceLookupError>();
}

struct FixtureContract;

impl CapabilityContract for FixtureContract {
    const ID: &'static str = "fixture";

    fn permissions() -> &'static [lockgate_policy::__private::ErasedPermission] {
        &[]
    }
}

#[test]
fn scope_repr_derive_supports_a_renamed_host_facade_only_dependency() {
    let manifest =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scope-reexport/Cargo.toml");
    let target =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/macro-diagnostic-fixtures");
    let output = Command::new(env!("CARGO"))
        .args(["check", "--manifest-path"])
        .arg(&manifest)
        .arg("--locked")
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("failed to check host scope re-export fixture");

    assert!(
        output.status.success(),
        "host scope re-export fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
