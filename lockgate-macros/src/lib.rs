use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use syn::{
    FnArg, GenericArgument, Ident, ImplItem, Item, ItemImpl, LitStr, PathArguments, ReturnType,
    Token, Type, TypePath, braced, parse::Parse, parse::ParseStream,
};
use wit_parser::{InterfaceId, Resolve, WorldId, WorldItem};

#[proc_macro]
pub fn bindings(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as BindingsInput);
    match expand_bindings(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

struct BindingsInput {
    path: LitStr,
    worlds: Vec<WorldSpec>,
}

struct WorldSpec {
    alias: Ident,
    world: LitStr,
}

impl Parse for BindingsInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut path = None;
        let mut worlds = None;
        while !input.is_empty() {
            let option: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match option.to_string().as_str() {
                "path" => {
                    if path.is_some() {
                        return Err(syn::Error::new(option.span(), "duplicate `path` option"));
                    }
                    path = Some(input.parse()?);
                }
                "worlds" => {
                    if worlds.is_some() {
                        return Err(syn::Error::new(option.span(), "duplicate `worlds` option"));
                    }
                    let content;
                    braced!(content in input);
                    let mut entries = Vec::new();
                    while !content.is_empty() {
                        let alias = content.parse()?;
                        content.parse::<Token![:]>()?;
                        let world = content.parse()?;
                        entries.push(WorldSpec { alias, world });
                        if content.is_empty() {
                            break;
                        }
                        content.parse::<Token![,]>()?;
                    }
                    worlds = Some(entries);
                }
                name => {
                    return Err(syn::Error::new(
                        option.span(),
                        format!("unsupported lockgate::bindings! option `{name}`"),
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let path = path.ok_or_else(|| {
            syn::Error::new(input.span(), "lockgate::bindings! requires a `path` string")
        })?;
        let worlds = worlds.ok_or_else(|| {
            syn::Error::new(input.span(), "lockgate::bindings! requires a `worlds` map")
        })?;
        if worlds.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "lockgate::bindings! requires at least one world",
            ));
        }
        let mut aliases = BTreeSet::new();
        for spec in &worlds {
            if !aliases.insert(spec.alias.to_string()) {
                return Err(syn::Error::new(spec.alias.span(), "duplicate binding name"));
            }
        }
        Ok(Self { path, worlds })
    }
}

struct SelectedWorld {
    alias: Ident,
    world: LitStr,
    id: WorldId,
    rust_name: Ident,
    module: Ident,
    exports: Vec<lockgate_schema::Export>,
}

struct ExportedInterface {
    accessor: Ident,
    module_path: Vec<Ident>,
}

fn expand_bindings(input: BindingsInput) -> syn::Result<TokenStream2> {
    let lockgate = lockgate_path(&input.path)?;
    let root = std::env::var("CARGO_MANIFEST_DIR").map_err(|error| {
        syn::Error::new_spanned(
            &input.path,
            format!("failed to locate package root: {error}"),
        )
    })?;
    let source = std::path::Path::new(&root).join(input.path.value());
    let mut resolve = Resolve::new();
    resolve.all_features = true;
    let (package, _) = resolve.push_path(&source).map_err(|error| {
        syn::Error::new_spanned(
            &input.path,
            format!("failed to parse {}: {error:#}", source.display()),
        )
    })?;
    let packages = [package];
    let mut selected = Vec::new();
    let mut imported_interfaces = BTreeMap::<String, InterfaceId>::new();
    for (index, spec) in input.worlds.into_iter().enumerate() {
        let id = resolve
            .select_world(&packages, Some(&spec.world.value()))
            .map_err(|error| syn::Error::new_spanned(&spec.world, error.to_string()))?;
        let world_name = &resolve.worlds[id].name;
        let rust_name = format_ident!(
            "{}",
            match world_name.as_str() {
                "host" => "Host_".into(),
                name => name.to_upper_camel_case(),
            }
        );
        let exports = lockgate_schema::world_exports(&resolve, id)
            .map_err(|error| syn::Error::new_spanned(&spec.world, error.to_string()))?;
        for item in resolve.worlds[id].imports.values() {
            let WorldItem::Interface { id, .. } = item else {
                continue;
            };
            let canonical = resolve.id_of(*id).ok_or_else(|| {
                syn::Error::new_spanned(
                    &spec.world,
                    "application worlds cannot share an unnamed imported interface",
                )
            })?;
            imported_interfaces.insert(canonical, *id);
        }
        selected.push(SelectedWorld {
            alias: spec.alias,
            world: spec.world,
            id,
            rust_name,
            module: format_ident!("__lockgate_world_{index}"),
            exports,
        });
    }

    let path = &input.path;
    let interface_declarations = imported_interfaces
        .keys()
        .map(|interface| format!("import {interface};"))
        .collect::<Vec<_>>()
        .join("\n");
    let interface_declarations = LitStr::new(&interface_declarations, input.path.span());
    let shared_bindings = if imported_interfaces.is_empty() {
        quote! {}
    } else {
        quote! {
            #[doc(hidden)]
            pub mod __lockgate_shared_imports {
                ::wasmtime::component::bindgen!({
                    path: #path,
                    interfaces: #interface_declarations,
                });
            }
        }
    };

    let namespaces = imported_interfaces
        .values()
        .filter_map(|id| resolve.interfaces[*id].package)
        .map(|id| rust_ident(&resolve.packages[id].name.namespace))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|namespace| format_ident!("{namespace}"));
    let shared_reexports = namespaces.map(|namespace| {
        quote! {
            pub use __lockgate_shared_imports::#namespace;
        }
    });

    let worlds = selected
        .iter()
        .map(|selected| -> syn::Result<TokenStream2> {
            let alias = &selected.alias;
            let world = &selected.world;
            let rust_name = &selected.rust_name;
            let module = &selected.module;
            let export_metadata = selected.exports.iter().map(|export| {
                let interface = &export.interface;
                let item = &export.item;
                let signature = &export.signature;
                quote! {
                    #lockgate::__private::BindingExport {
                        interface: #interface,
                        item: #item,
                        signature: #signature,
                    }
                }
            });
            let host_imports = resolve.worlds[selected.id]
                .imports
                .values()
                .filter_map(|item| match item {
                    WorldItem::Interface { id, .. } => {
                        Some((resolve.id_of(*id).expect("validated named import"), *id))
                    }
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            let wasmtime_with = host_imports
                .iter()
                .map(|(canonical, id)| {
                    let path = interface_module_path(&resolve, *id)
                        .into_iter()
                        .map(|segment| segment.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    (
                        canonical.clone(),
                        format!("super::__lockgate_shared_imports::{path}"),
                    )
                })
                .collect::<HashMap<_, _>>();
            let client =
                generate_runtime_client(&source, &resolve, selected, wasmtime_with, &lockgate)?;
            let generated_client_name = format_ident!("{}Client", selected.rust_name);
            let host_bounds = host_imports
                .values()
                .map(|id| {
                    let path = interface_module_path(&resolve, *id);
                    quote! {
                        #lockgate::HostContext<S>:
                            super::__lockgate_shared_imports::#(#path)::*::Host
                            + super::__lockgate_shared_imports::#(#path)::*::HostWithStore<
                                #lockgate::__private::PluginStore<S>
                            >,
                        for<'a> <#lockgate::HostContext<S> as ::wasmtime::component::HasData>::Data<'a>:
                            super::__lockgate_shared_imports::#(#path)::*::Host,
                    }
                })
                .collect::<Vec<_>>();
            let installers = host_imports.iter().enumerate().map(|(index, (_, id))| {
                let installer = format_ident!("__lockgate_install_host_import_{index}");
                let path = interface_module_path(&resolve, *id);
                quote! {
                    fn #installer<S: Send + Sync + 'static>(
                        linker: &mut ::wasmtime::component::Linker<
                            #lockgate::__private::PluginStore<S>,
                        >,
                    ) -> #lockgate::__private::AnyResult<()>
                    where
                        #lockgate::HostContext<S>:
                            super::__lockgate_shared_imports::#(#path)::*::Host
                            + super::__lockgate_shared_imports::#(#path)::*::HostWithStore<
                                #lockgate::__private::PluginStore<S>
                            >,
                        for<'a> <#lockgate::HostContext<S> as ::wasmtime::component::HasData>::Data<'a>:
                            super::__lockgate_shared_imports::#(#path)::*::Host,
                    {
                        super::__lockgate_shared_imports::#(#path)::*::add_to_linker::<
                            _,
                            #lockgate::HostContext<S>,
                        >(linker, #lockgate::__private::host_context_data::<S>)?;
                        Ok(())
                    }
                }
            });
            let installer_arms = host_imports.keys().enumerate().map(|(index, interface)| {
                let installer = format_ident!("__lockgate_install_host_import_{index}");
                quote! {
                    #interface => #installer::<S>(linker),
                }
            });
            let host_import_names = host_imports.keys();
            let host_context_data = quote! {};
            let remappings = resolve.worlds[selected.id]
                .imports
                .iter()
                .filter_map(|(key, item)| match item {
                    WorldItem::Interface { id, .. } => Some((key, *id)),
                    _ => None,
                })
                .map(|(key, id)| {
                    let lookup = match key {
                        wit_parser::WorldKey::Name(name) => name.clone(),
                        wit_parser::WorldKey::Interface(_) => {
                            resolve.id_of(id).expect("validated named import")
                        }
                    };
                    let lookup = LitStr::new(&lookup, world.span());
                    let path = interface_module_path(&resolve, id);
                    quote! {
                        #lookup: super::__lockgate_shared_imports::#(#path)::*
                    }
                });
            Ok(quote! {
                #[doc(hidden)]
                pub mod #module {
                    ::wasmtime::component::bindgen!({
                        path: #path,
                        world: #world,
                        with: { #(#remappings),* },
                        require_store_data_send: true,
                    });

                    const _: () = {
                        #host_context_data
                        #(#installers)*

                        impl<S: Send + Sync + 'static> #lockgate::__private::Binding<S> for #rust_name
                        where
                            #(#host_bounds)*
                        {
                            const WORLD: &'static str = #world;
                            const EXPORTS: &'static [#lockgate::__private::BindingExport] =
                                &[#(#export_metadata),*];
                            const HOST_IMPORTS: &'static [&'static str] =
                                &[#(#host_import_names),*];

                            type Client<'runtime> = #generated_client_name<'runtime, S>
                            where
                                S: 'runtime;

                            fn bind(
                                accessor: &::wasmtime::component::Accessor<
                                    #lockgate::__private::PluginStore<S>
                                >,
                                instance: &::wasmtime::component::Instance,
                            ) -> #lockgate::__private::AnyResult<Self> {
                                accessor.with(|mut access| Ok(Self::new(&mut access, instance)?))
                            }

                            fn install_host_import(
                                interface: &str,
                                linker: &mut ::wasmtime::component::Linker<
                                    #lockgate::__private::PluginStore<S>,
                                >,
                            ) -> #lockgate::__private::AnyResult<()> {
                                match interface {
                                    #(#installer_arms)*
                                    _ => Err(::wasmtime::Error::msg(
                                        "host interface is not part of this binding role",
                                    ).into()),
                                }
                            }

                            fn client<'runtime>(
                                component: #lockgate::__private::RuntimeComponent<
                                    'runtime,
                                    S,
                                    Self,
                                >,
                            ) -> Self::Client<'runtime>
                            where
                                S: 'runtime,
                            {
                                #generated_client_name { inner: component }
                            }
                        }

                        impl<S: Send + Sync + 'static> #lockgate::__private::RoleSet<S> for #rust_name
                        where
                            #(#host_bounds)*
                        {
                            type Handles = #lockgate::Component<Self>;

                            #[allow(clippy::type_complexity)]
                            fn for_each_role(
                                visitor: &mut dyn FnMut(
                                    &'static str,
                                    &'static [#lockgate::__private::BindingExport],
                                    &'static [&'static str],
                                    fn(
                                        &str,
                                        &mut ::wasmtime::component::Linker<
                                            #lockgate::__private::PluginStore<S>,
                                        >,
                                    ) -> #lockgate::__private::AnyResult<()>,
                                ),
                            ) {
                                visitor(
                                    <Self as #lockgate::__private::Binding<S>>::WORLD,
                                    <Self as #lockgate::__private::Binding<S>>::EXPORTS,
                                    <Self as #lockgate::__private::Binding<S>>::HOST_IMPORTS,
                                    <Self as #lockgate::__private::Binding<S>>::install_host_import,
                                );
                            }

                            fn handles(component: #lockgate::Component<Self>) -> Self::Handles {
                                component
                            }
                        }
                    };

                    #client
                }

                pub use #module::#rust_name as #alias;
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        #shared_bindings
        #(#shared_reexports)*
        #(#worlds)*
    })
}

fn generate_runtime_client(
    source: &std::path::Path,
    resolve: &Resolve,
    selected: &SelectedWorld,
    with: HashMap<String, String>,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let mut wasmtime_resolve = wasmtime_wit_parser::Resolve::new();
    wasmtime_resolve.all_features = true;
    let (package, _) = wasmtime_resolve.push_path(source).map_err(|error| {
        syn::Error::new_spanned(
            &selected.world,
            format!(
                "failed to prepare Wasmtime client signatures from {}: {error:#}",
                source.display()
            ),
        )
    })?;
    let world = wasmtime_resolve
        .select_world(&[package], Some(&selected.world.value()))
        .map_err(|error| syn::Error::new_spanned(&selected.world, error.to_string()))?;
    let mut options = wasmtime_wit_bindgen::Opts {
        with,
        ..Default::default()
    };
    options.wasmtime_crate = Some("::wasmtime".into());
    let generated = options
        .generate(&mut wasmtime_resolve, world)
        .map_err(|error| syn::Error::new_spanned(&selected.world, error.to_string()))?;
    let generated = syn::parse_file(&generated).map_err(|error| {
        syn::Error::new_spanned(
            &selected.world,
            format!("failed to read generated Wasmtime signatures: {error}"),
        )
    })?;

    if resolve.worlds[selected.id].exports.len() != 1 {
        return Err(syn::Error::new_spanned(
            &selected.world,
            "Lockgate binding roles must export exactly one interface; add multi-interface components with a tuple of narrow roles",
        ));
    }
    let mut exported = Vec::new();
    for (key, item) in &resolve.worlds[selected.id].exports {
        let WorldItem::Interface { id, .. } = item else {
            return Err(syn::Error::new_spanned(
                &selected.world,
                "runtime clients require interface exports",
            ));
        };
        let (accessor, module_path) = match key {
            wit_parser::WorldKey::Name(name) => {
                let name = rust_ident(name);
                (
                    format_ident!("{name}"),
                    vec![format_ident!("exports"), format_ident!("{name}")],
                )
            }
            wit_parser::WorldKey::Interface(_) => {
                let path = interface_module_path(resolve, *id);
                let accessor = path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("_");
                let mut module_path = vec![format_ident!("exports")];
                module_path.extend(path);
                (format_ident!("{accessor}"), module_path)
            }
        };
        exported.push(ExportedInterface {
            accessor,
            module_path,
        });
    }

    let rust_name = &selected.rust_name;
    let world_client = format_ident!("{rust_name}Client");
    let host_bounds = resolve.worlds[selected.id]
        .imports
        .values()
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => Some(interface_module_path(resolve, *id)),
            _ => None,
        })
        .map(|path| {
            quote! {
                #lockgate::HostContext<S>:
                    super::__lockgate_shared_imports::#(#path)::*::Host
                    + super::__lockgate_shared_imports::#(#path)::*::HostWithStore<
                        #lockgate::__private::PluginStore<S>
                    >,
                for<'a> <#lockgate::HostContext<S> as ::wasmtime::component::HasData>::Data<'a>:
                    super::__lockgate_shared_imports::#(#path)::*::Host,
            }
        })
        .collect::<Vec<_>>();
    let interface = exported.pop().expect("one exported interface was required");
    let module_items =
        nested_module_items(&generated.items, &interface.module_path).ok_or_else(|| {
            syn::Error::new_spanned(
                &selected.world,
                format!(
                    "could not locate generated export module `{}`",
                    interface
                        .module_path
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("::")
                ),
            )
        })?;
    let client = generate_interface_client(
        module_items,
        &interface,
        rust_name,
        &world_client,
        &host_bounds,
        lockgate,
    )?;

    Ok(quote! {
        pub struct #world_client<'runtime, S: Send + Sync + 'static> {
            inner: #lockgate::__private::RuntimeComponent<'runtime, S, #rust_name>,
        }

        #client

    })
}

fn nested_module_items<'a>(items: &'a [Item], path: &[Ident]) -> Option<&'a [Item]> {
    let mut items = items;
    for segment in path {
        let module = items.iter().find_map(|item| match item {
            Item::Mod(module) if module.ident == *segment => Some(module),
            _ => None,
        })?;
        items = &module.content.as_ref()?.1;
    }
    Some(items)
}

fn generate_interface_client(
    items: &[Item],
    interface: &ExportedInterface,
    rust_name: &Ident,
    world_client: &Ident,
    host_bounds: &[TokenStream2],
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let mut calls = BTreeMap::<String, Vec<syn::ImplItemFn>>::new();
    let mut resources = BTreeMap::<String, Ident>::new();
    for item in items {
        let Item::Impl(item) = item else {
            continue;
        };
        let Some(target) = impl_target(item) else {
            continue;
        };
        if !target.starts_with("Guest") {
            continue;
        }
        for impl_item in &item.items {
            let ImplItem::Fn(function) = impl_item else {
                continue;
            };
            if function.sig.ident.to_string().starts_with("call_") {
                calls
                    .entry(target.clone())
                    .or_default()
                    .push(function.clone());
            } else if target == "Guest"
                && function.sig.inputs.len() == 1
                && let ReturnType::Type(_, ty) = &function.sig.output
                && let Some(resource) = type_path_last_ident(ty)
                && resource.to_string().starts_with("Guest")
            {
                resources.insert(resource.to_string(), function.sig.ident.clone());
            }
        }
    }

    let module_path = &interface.module_path;
    let export_path = quote!(#(#module_path)::*);
    let root_accessor = &interface.accessor;
    let direct_calls = calls.remove("Guest").unwrap_or_default();
    let direct_methods = direct_calls
        .iter()
        .map(|function| generate_client_method(function, root_accessor, None, lockgate))
        .collect::<syn::Result<Vec<_>>>()?;

    let mut resource_structs = Vec::new();
    let mut resource_accessors = Vec::new();
    for (guest, functions) in calls {
        let Some(accessor) = resources.get(&guest) else {
            return Err(syn::Error::new_spanned(
                &interface.accessor,
                format!("could not locate generated resource accessor for `{guest}`"),
            ));
        };
        let resource_name = guest.strip_prefix("Guest").unwrap_or(&guest);
        let resource_client = format_ident!("{}Client", resource_name);
        let methods = functions
            .iter()
            .map(|function| {
                generate_client_method(function, root_accessor, Some(accessor), lockgate)
            })
            .collect::<syn::Result<Vec<_>>>()?;
        resource_accessors.push(quote! {
            pub fn #accessor(&self) -> #resource_client<'runtime, S> {
                #resource_client { inner: self.inner }
            }
        });
        resource_structs.push(quote! {
            pub struct #resource_client<'runtime, S: Send + Sync + 'static> {
                inner: #lockgate::__private::RuntimeComponent<'runtime, S, #rust_name>,
            }

            impl<'runtime, S: Send + Sync + 'static> #resource_client<'runtime, S>
            where
                #(#host_bounds)*
            {
                #(#methods)*
            }
        });
    }

    Ok(quote! {
        #[allow(unused_imports)]
        use self::#export_path::*;

        impl<'runtime, S: Send + Sync + 'static> #world_client<'runtime, S>
        where
            #(#host_bounds)*
        {
            #(#direct_methods)*
            #(#resource_accessors)*
        }

        #(#resource_structs)*
    })
}

fn impl_target(item: &ItemImpl) -> Option<String> {
    let Type::Path(TypePath { path, .. }) = item.self_ty.as_ref() else {
        return None;
    };
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn type_path_last_ident(ty: &Type) -> Option<&Ident> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    path.segments.last().map(|segment| &segment.ident)
}

fn generate_client_method(
    function: &syn::ImplItemFn,
    root_accessor: &Ident,
    resource_accessor: Option<&Ident>,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let async_call = function.sig.asyncness.is_some();
    let original = &function.sig.ident;
    let Some(name) = original
        .to_string()
        .strip_prefix("call_")
        .map(str::to_owned)
    else {
        return Err(syn::Error::new_spanned(
            original,
            "generated guest call is missing the `call_` prefix",
        ));
    };
    let name = format_ident!("{name}");
    let mut inputs = function.sig.inputs.iter();
    let Some(receiver) = inputs.next() else {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "generated guest call is missing its receiver",
        ));
    };
    let Some(_store) = inputs.next() else {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "generated guest call is missing its store parameter",
        ));
    };
    let params = inputs.cloned().collect::<Vec<_>>();
    let args = params
        .iter()
        .map(|param| match param {
            FnArg::Typed(param) => match param.pat.as_ref() {
                syn::Pat::Ident(ident) => Ok(ident.ident.clone()),
                pattern => Err(syn::Error::new_spanned(
                    pattern,
                    "generated guest parameter must use an identifier pattern",
                )),
            },
            FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(
                receiver,
                "generated guest call has an unexpected receiver parameter",
            )),
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let owned_args = params.iter().zip(&args).filter_map(|(param, arg)| {
        let FnArg::Typed(param) = param else {
            return None;
        };
        matches!(param.ty.as_ref(), Type::Reference(_)).then(|| {
            quote! {
                let #arg = #arg.to_owned();
            }
        })
    });
    let call_args = params.iter().zip(&args).map(|(param, arg)| {
        let FnArg::Typed(param) = param else {
            unreachable!("generated parameters were validated as typed arguments");
        };
        if matches!(param.ty.as_ref(), Type::Reference(_)) {
            quote!(&#arg)
        } else {
            quote!(#arg)
        }
    });
    let result = generated_result_type(&function.sig.output)?;
    let guest = if let Some(resource) = resource_accessor {
        quote!(binding.#root_accessor().#resource())
    } else {
        quote!(binding.#root_accessor())
    };
    let call = if async_call {
        quote! {
            #guest.#original(accessor, #(#call_args),*).await
        }
    } else {
        quote! {
            accessor.with(|mut access| {
                #guest.#original(&mut access, #(#call_args),*)
            })
        }
    };
    Ok(quote! {
        pub async fn #name(#receiver, #(#params),*)
            -> ::std::result::Result<#result, #lockgate::RuntimeError>
        {
            #(#owned_args)*
            self.inner.invoke(move |accessor, binding| {
                ::std::boxed::Box::pin(async move {
                    Ok(#call?)
                })
            }).await
        }
    })
}

fn generated_result_type(output: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "generated guest call does not return a Wasmtime result",
        ));
    };
    let Type::Path(TypePath { path, .. }) = ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            ty,
            "generated guest call uses an unsupported return type",
        ));
    };
    let Some(segment) = path.segments.last() else {
        return Err(syn::Error::new_spanned(
            ty,
            "generated result path is empty",
        ));
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            segment,
            "generated Wasmtime result is missing its value type",
        ));
    };
    arguments
        .args
        .iter()
        .find_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            syn::Error::new_spanned(arguments, "generated Wasmtime result has no value type")
        })
}

fn interface_module_path(resolve: &Resolve, interface: InterfaceId) -> Vec<Ident> {
    let interface = &resolve.interfaces[interface];
    let package_id = interface.package.expect("named interface has a package");
    let package = &resolve.packages[package_id];
    let same_name_versions = resolve
        .packages
        .iter()
        .filter(|(_, candidate)| {
            candidate.name.namespace == package.name.namespace
                && candidate.name.name == package.name.name
        })
        .count();
    let mut package_module = package.name.name.to_snake_case();
    if same_name_versions > 1
        && let Some(version) = &package.name.version
    {
        let version = version
            .to_string()
            .replace(['.', '-', '+'], "_")
            .to_snake_case();
        package_module.push_str(&version);
    }
    [
        rust_ident(&package.name.namespace),
        rust_ident(&package_module),
        rust_ident(interface.name.as_deref().expect("named interface")),
    ]
    .into_iter()
    .map(|name| format_ident!("{name}"))
    .collect()
}

fn rust_ident(name: &str) -> String {
    match name {
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false"
        | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move"
        | "mut" | "pub" | "ref" | "return" | "self" | "static" | "struct" | "super" | "trait"
        | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await" | "dyn"
        | "abstract" | "become" | "box" | "do" | "final" | "macro" | "override" | "priv"
        | "typeof" | "unsized" | "virtual" | "yield" | "try" | "gen" => format!("{name}_"),
        name => name.to_snake_case(),
    }
}

fn lockgate_path(span: impl quote::ToTokens) -> syn::Result<TokenStream2> {
    match crate_name("lockgate") {
        Ok(FoundCrate::Itself) => Ok(quote!(crate)),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            Ok(quote!(::#name))
        }
        Err(error) => Err(syn::Error::new_spanned(
            span,
            format!("failed to resolve the lockgate crate: {error}"),
        )),
    }
}
