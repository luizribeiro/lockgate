//! Private preflight validation and provider resolution for runtime construction.
//! Every application import is authorized and structurally checked before any store is created.

use crate::{
    catalog::{Catalog, ComponentId, ExportInfo},
    grants::{DirectoryGrant, Grants},
    http::HttpOrigin,
    plugin::validate_cross_store_signature,
    runtime::RuntimeBuildError,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) component: ComponentId,
    pub(crate) plugin_id: String,
    pub(crate) interface: String,
    pub(crate) function: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedImport {
    pub(crate) functions: Vec<(String, Target)>,
}

#[derive(Default)]
pub(crate) struct ComponentPlan {
    pub(crate) direct_imports: HashMap<String, ResolvedImport>,
    pub(crate) host_imports: HashSet<String>,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) outbound_http: Option<Vec<HttpOrigin>>,
}

/// An application catalog and its grants compiled into deterministic provider selections.
pub(crate) struct Plan {
    pub(crate) catalog: Catalog,
    pub(crate) components: HashMap<ComponentId, ComponentPlan>,
    pub(crate) order: Vec<ComponentId>,
}

impl Plan {
    /// Resolves all retained grants against the catalog's decoded WIT.
    pub(crate) fn new(catalog: Catalog, grants: Grants) -> Result<Self, RuntimeBuildError> {
        let order = catalog.component_ids();
        let mut components = order
            .iter()
            .copied()
            .map(|id| (id, ComponentPlan::default()))
            .collect::<HashMap<_, _>>();

        for grant in &grants.host_imports {
            catalog
                .entry(grant.component)
                .expect("host grants contain only validated component handles");
            components
                .get_mut(&grant.component)
                .expect("application contains every host grant component")
                .host_imports
                .insert(grant.interface.clone());
        }
        for grant in &grants.directories {
            let entry = catalog
                .entry(grant.component)
                .expect("directory grants contain only validated component handles");
            let plan = components
                .get_mut(&grant.component)
                .expect("application contains every directory grant component");
            if plan
                .directories
                .iter()
                .any(|existing| existing.guest == grant.guest)
            {
                return Err(RuntimeBuildError::DuplicateGuestDirectory {
                    component: entry.metadata.id().into(),
                    guest: grant.guest.display().to_string(),
                });
            }
            plan.directories.push(grant.clone());
        }
        for grant in &grants.outbound_http {
            let plan = components
                .get_mut(&grant.component)
                .expect("application contains every outbound HTTP grant component");
            match &mut plan.outbound_http {
                Some(origins) => {
                    for origin in &grant.origins {
                        if !origins.contains(origin) {
                            origins.push(origin.clone());
                        }
                    }
                }
                None => plan.outbound_http = Some(grant.origins.clone()),
            }
        }
        let links = grants
            .links
            .iter()
            .map(|grant| (grant.caller, grant.provider))
            .collect::<Vec<_>>();
        for caller in &order {
            let caller = *caller;
            let caller_entry = catalog
                .entry(caller)
                .expect("application contains only validated component handles");
            for import in &caller_entry.direct_imports {
                if components
                    .get(&caller)
                    .unwrap()
                    .host_imports
                    .contains(&import.interface)
                {
                    continue;
                }
                let providers = links
                    .iter()
                    .filter(|(linked_caller, _)| *linked_caller == caller)
                    .map(|(_, provider)| *provider)
                    .filter(|provider| {
                        catalog
                            .entry(*provider)
                            .expect("links contain only validated provider handles")
                            .exports
                            .iter()
                            .any(|export| export.interface == import.interface)
                    })
                    .collect::<HashSet<_>>();
                if providers.is_empty() {
                    return Err(RuntimeBuildError::MissingProvider {
                        caller: caller_entry.metadata.id().into(),
                        interface: import.interface.clone(),
                    });
                }
                if providers.len() > 1 {
                    return Err(RuntimeBuildError::AmbiguousProvider {
                        caller: caller_entry.metadata.id().into(),
                        interface: import.interface.clone(),
                    });
                }
                let provider = *providers.iter().next().unwrap();
                let provider_entry = catalog
                    .entry(provider)
                    .expect("selected providers come from validated links");
                let mut functions = Vec::new();
                for (function, expected) in &import.functions {
                    let target_name = format!("{}#{function}", import.interface);
                    validate_cross_store_signature(expected).map_err(|error| {
                        RuntimeBuildError::UnsupportedSiblingType {
                            target: target_name.clone(),
                            reason: error.to_string(),
                        }
                    })?;
                    let export = provider_entry
                        .exports
                        .iter()
                        .find(|export| export.target == target_name)
                        .ok_or_else(|| RuntimeBuildError::MissingFunction {
                            caller: caller_entry.metadata.id().into(),
                            provider: provider_entry.metadata.id().into(),
                            target: target_name.clone(),
                        })?;
                    validate_cross_store_signature(&export.runtime_signature).map_err(|error| {
                        RuntimeBuildError::UnsupportedSiblingType {
                            target: target_name.clone(),
                            reason: error.to_string(),
                        }
                    })?;
                    if &export.runtime_signature != expected {
                        return Err(RuntimeBuildError::TypeMismatch {
                            target: target_name,
                            expected: expected.to_string(),
                            actual: export.runtime_signature.to_string(),
                        });
                    }
                    functions.push((
                        function.clone(),
                        target_from_info(provider, provider_entry.metadata.id().into(), export),
                    ));
                }
                components
                    .get_mut(&caller)
                    .unwrap()
                    .direct_imports
                    .insert(import.interface.clone(), ResolvedImport { functions });
            }
        }

        Ok(Self {
            catalog,
            components,
            order,
        })
    }
}

fn target_from_info(component: ComponentId, plugin_id: String, export: &ExportInfo) -> Target {
    Target {
        component,
        plugin_id,
        interface: export.interface.clone(),
        function: export.function.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grants::{Grants, LinkGrant, OutboundHttpGrant};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    fn component_bytes(wit: &str, world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve.push_str("plan.wit", wit).unwrap();
        let world = resolve.packages[package].worlds[world_name];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        let bytes = ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap();
        crate::catalog::with_test_plugin_metadata(bytes, world_name)
    }

    #[test]
    fn resolves_authorized_typed_imports() {
        let wit = r#"package demo:plan@0.1.0;
interface api { run: func(input: string) -> string; }
world provider { export api; }
world caller { import api; }"#;
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog.add_untyped(component_bytes(wit, "caller")).unwrap();
        let provider = catalog
            .add_untyped(component_bytes(wit, "provider"))
            .unwrap();
        let mut grants = Grants::default();
        grants.links.push(LinkGrant { caller, provider });
        let plan = Plan::new(catalog, grants).unwrap();
        assert!(
            plan.components
                .get(&caller)
                .unwrap()
                .direct_imports
                .contains_key("demo:plan/api@0.1.0")
        );
    }

    #[test]
    fn retains_outbound_http_grants_per_component() {
        let wit = r#"package wasi:http@0.3.0;
interface client { send: func(); }
world caller { import client; }
world other {}"#;
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog.add_untyped(component_bytes(wit, "caller")).unwrap();
        let other = catalog.add_untyped(component_bytes(wit, "other")).unwrap();
        let mut grants = Grants::default();
        grants.outbound_http.push(OutboundHttpGrant {
            component: caller,
            origins: vec![HttpOrigin::parse("https://example.com").unwrap()],
        });

        let plan = Plan::new(catalog, grants).unwrap();

        assert_eq!(
            plan.components[&caller].outbound_http,
            Some(vec![HttpOrigin::parse("https://example.com").unwrap()])
        );
        assert_eq!(plan.components[&other].outbound_http, None);
    }
}
