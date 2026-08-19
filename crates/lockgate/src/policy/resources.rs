//! Application-owned resource classification and target resolution.

use std::{future::Future, str::FromStr};

use lockgate_policy::{Scope, ScopeError};

use crate::PluginHandle;

/// The admitted plugin identity making one host invocation.
///
/// Lockgate constructs this value from the invocation's [`PluginHandle`]
/// before resolving or classifying a scoped target. Keeping the subject as a
/// dedicated value leaves room for future stable subject attributes without
/// making application policy depend on Lockgate's full plugin handle.
#[derive(Clone, Copy, Debug)]
pub struct PluginSubject<'a> {
    plugin: &'a PluginHandle,
}

impl<'a> PluginSubject<'a> {
    #[allow(
        dead_code,
        reason = "guard expansion constructs subjects from invocation plugin handles in the next policy chunk"
    )]
    pub(crate) const fn new(plugin: &'a PluginHandle) -> Self {
        Self { plugin }
    }

    /// Returns the stable ID of the plugin making this invocation.
    pub fn plugin_id(&self) -> &str {
        self.plugin.id()
    }
}

/// Classifies an application-owned resource into scope membership witnesses.
///
/// Implementations must report every concrete witness needed to preserve each
/// authority distinction promised by `S`. For every grant meant to authorize
/// this resource, at least one returned witness must be contained by that
/// grant. In particular, a broad grant does not authorize an empty result:
/// returning an empty vector deliberately denies access (fail closed).
///
/// Classification is pure and synchronous. Perform fallible or asynchronous
/// lookup through [`ResolveScopedResource`] and return an owned authorization
/// snapshot when classification needs facts loaded from external state.
/// Duplicate witnesses are permitted and do not change authorization's
/// existential containment relation.
pub trait ScopedResource<S: Scope>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// Returns all scope membership witnesses for `self` and `subject`.
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<S>;
}

/// Resolves a host-call target expression to an application-owned resource.
///
/// Resolution is fallible and may be asynchronous; the returned resource's
/// [`ScopedResource`] implementation performs the separate, pure
/// classification step. Rust selects a resolver from both the permission's
/// scope type `S` and the target expression type `A`.
///
/// The explicit `impl Future + Send` return matches Lockgate's generated async
/// host boundary while allowing implementations to use native `async fn`
/// syntax without boxing.
pub trait ResolveScopedResource<S, A: ?Sized + Sync>: Send + Sync
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// The owned resource or authorization snapshot produced by resolution.
    type Resource: ScopedResource<S> + Send;

    /// The application's domain error for failed resolution.
    type Error: Send;

    /// Resolves `argument` for the calling plugin.
    fn resolve_scoped_resource<'a>(
        &'a self,
        subject: &'a PluginSubject<'_>,
        argument: &'a A,
    ) -> impl Future<Output = Result<Self::Resource, Self::Error>> + Send + 'a;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
    enum TestScope {
        Member,
    }

    impl Scope for TestScope {}

    struct TestResource;

    impl ScopedResource<TestScope> for TestResource {
        fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<TestScope> {
            vec![TestScope::Member]
        }
    }

    struct TestResolver;

    impl ResolveScopedResource<TestScope, str> for TestResolver {
        type Resource = TestResource;
        type Error = ();

        async fn resolve_scoped_resource<'a>(
            &'a self,
            _subject: &'a PluginSubject<'_>,
            _argument: &'a str,
        ) -> Result<Self::Resource, Self::Error> {
            Ok(TestResource)
        }
    }

    #[test]
    fn resolver_impl_uses_native_async_fn_with_send_boundary_types() {
        fn assert_resolver<T>()
        where
            T: ResolveScopedResource<TestScope, str>,
            T::Resource: Send,
            T::Error: Send,
        {
        }

        assert_resolver::<TestResolver>();
    }
}
