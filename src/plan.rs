//! Private preflight validation and provider resolution for runtime construction.
//! Every included import is authorized and structurally checked before any store is created.

use crate::{
    REGISTRY_INTERFACE,
    catalog::{Catalog, CatalogError, ComponentId, ExportInfo},
    plugin::Signature,
    policy::{DirectoryGrant, Policy},
    runtime::RuntimeBuildError,
};
use std::collections::{HashMap, HashSet};
use wasmtime::component::types::Type;

#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) component: ComponentId,
    pub(crate) component_name: String,
    pub(crate) interface: String,
    pub(crate) function: String,
    pub(crate) signature: Signature,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedImport {
    pub(crate) interface: String,
    pub(crate) functions: Vec<(String, Target)>,
}

#[derive(Default)]
pub(crate) struct ComponentPlan {
    pub(crate) direct_imports: HashMap<String, ResolvedImport>,
    pub(crate) lookups: HashMap<String, Target>,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) registry: bool,
}

/// A catalog and policy compiled into deterministic provider selections.
pub(crate) struct Plan {
    pub(crate) catalog: Catalog,
    pub(crate) components: HashMap<ComponentId, ComponentPlan>,
    pub(crate) order: Vec<ComponentId>,
}

impl Plan {
    /// Resolves all authority granted by a policy against the catalog's decoded WIT.
    pub(crate) fn new(catalog: Catalog, policy: Policy) -> Result<Self, RuntimeBuildError> {
        if policy.catalog_identity() != catalog.identity() {
            return Err(RuntimeBuildError::ForeignPolicy);
        }

        let mut components = policy
            .components()
            .iter()
            .copied()
            .map(|id| (id, ComponentPlan::default()))
            .collect::<HashMap<_, _>>();

        for component in policy.registries() {
            catalog.entry(*component)?;
            components.get_mut(component).unwrap().registry = true;
        }
        for grant in policy.directories() {
            let entry = catalog.entry(grant.component())?;
            let plan = components.get_mut(&grant.component()).unwrap();
            if plan
                .directories
                .iter()
                .any(|existing| existing.guest() == grant.guest())
            {
                return Err(RuntimeBuildError::DuplicateGuestDirectory {
                    component: entry.name.clone(),
                    guest: grant.guest().display().to_string(),
                });
            }
            plan.directories.push(grant.clone());
        }
        for grant in policy.lookups() {
            let target = target_from_export(&catalog, grant.provider(), grant.target())?;
            if !dynamic_signature_supported(&target.signature) {
                return Err(RuntimeBuildError::UnsupportedDynamicType {
                    target: target.key(),
                });
            }
            let caller_name = catalog.entry(grant.caller())?.name.clone();
            let lookups = &mut components
                .get_mut(&grant.caller())
                .ok_or(CatalogError::ForeignComponent)?
                .lookups;
            if lookups.insert(target.key(), target.clone()).is_some() {
                return Err(RuntimeBuildError::AmbiguousLookup {
                    caller: caller_name,
                    target: target.key(),
                });
            }
        }

        let links = policy
            .links()
            .iter()
            .map(|grant| (grant.caller(), grant.provider()))
            .collect::<Vec<_>>();
        for caller in policy.components() {
            let caller = *caller;
            let caller_entry = catalog.entry(caller)?;
            if caller_entry
                .imports
                .iter()
                .any(|import| import == REGISTRY_INTERFACE)
                && !components.get(&caller).unwrap().registry
            {
                return Err(RuntimeBuildError::RegistryNotEnabled {
                    component: caller_entry.name.clone(),
                });
            }
            for import in &caller_entry.direct_imports {
                let providers = links
                    .iter()
                    .filter(|(linked_caller, _)| *linked_caller == caller)
                    .map(|(_, provider)| *provider)
                    .filter(|provider| {
                        catalog.entry(*provider).is_ok_and(|entry| {
                            entry
                                .exports
                                .iter()
                                .any(|export| export.interface() == import.interface)
                        })
                    })
                    .collect::<HashSet<_>>();
                if providers.is_empty() {
                    return Err(RuntimeBuildError::MissingProvider {
                        caller: caller_entry.name.clone(),
                        interface: import.interface.clone(),
                    });
                }
                if providers.len() > 1 {
                    return Err(RuntimeBuildError::AmbiguousProvider {
                        caller: caller_entry.name.clone(),
                        interface: import.interface.clone(),
                    });
                }
                let provider = *providers.iter().next().unwrap();
                let provider_entry = catalog.entry(provider)?;
                let mut functions = Vec::new();
                for (function, expected) in &import.functions {
                    let target_name = format!("{}#{function}", import.interface);
                    let export = provider_entry
                        .exports
                        .iter()
                        .find(|export| export.target() == target_name)
                        .ok_or_else(|| RuntimeBuildError::MissingFunction {
                            caller: caller_entry.name.clone(),
                            provider: provider_entry.name.clone(),
                            target: target_name.clone(),
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
                        target_from_info(provider, provider_entry.name.clone(), export),
                    ));
                }
                components.get_mut(&caller).unwrap().direct_imports.insert(
                    import.interface.clone(),
                    ResolvedImport {
                        interface: import.interface.clone(),
                        functions,
                    },
                );
            }
        }

        Ok(Self {
            catalog,
            components,
            order: policy.components().to_vec(),
        })
    }

    pub(crate) fn component(&self, id: ComponentId) -> Result<&ComponentPlan, CatalogError> {
        self.catalog.entry(id)?;
        self.components
            .get(&id)
            .ok_or(CatalogError::ForeignComponent)
    }

    pub(crate) fn target(
        &self,
        component: ComponentId,
        target: &str,
    ) -> Result<Target, CatalogError> {
        target_from_export(&self.catalog, component, target)
    }
}

fn dynamic_signature_supported(signature: &Signature) -> bool {
    signature
        .params
        .iter()
        .chain(&signature.results)
        .all(|ty| match ty {
            Type::Bool | Type::S32 | Type::U32 | Type::String => true,
            Type::List(list) => list.ty() == Type::String,
            _ => false,
        })
}

impl Target {
    pub(crate) fn key(&self) -> String {
        format!("{}#{}", self.interface, self.function)
    }
}

fn target_from_export(
    catalog: &Catalog,
    component: ComponentId,
    target: &str,
) -> Result<Target, CatalogError> {
    let entry = catalog.entry(component)?;
    Ok(target_from_info(
        component,
        entry.name.clone(),
        catalog.export(component, target)?,
    ))
}

fn target_from_info(component: ComponentId, component_name: String, export: &ExportInfo) -> Target {
    Target {
        component,
        component_name,
        interface: export.interface().into(),
        function: export.function().into(),
        signature: export.runtime_signature.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Policy;
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    fn component_bytes(wit: &str, world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve.push_str("plan.wit", wit).unwrap();
        let world = resolve.packages[package].worlds[world_name];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
    }

    #[test]
    fn resolves_authorized_typed_imports() {
        let wit = r#"package demo:plan@0.1.0;
interface api { run: func(input: string) -> string; }
world provider { export api; }
world caller { import api; }"#;
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add("caller", component_bytes(wit, "caller"))
            .unwrap();
        let provider = catalog
            .add("provider", component_bytes(wit, "provider"))
            .unwrap();
        let policy = Policy::builder(&catalog)
            .link(caller, provider)
            .unwrap()
            .build();
        let plan = Plan::new(catalog, policy).unwrap();
        assert!(
            plan.component(caller)
                .unwrap()
                .direct_imports
                .contains_key("demo:plan/api@0.1.0")
        );
    }

    #[test]
    fn rejects_a_policy_from_another_catalog() {
        let first = Catalog::new().unwrap();
        let policy = Policy::builder(&first).build();
        let second = Catalog::new().unwrap();
        assert!(matches!(
            Plan::new(second, policy),
            Err(RuntimeBuildError::ForeignPolicy)
        ));
    }
}
