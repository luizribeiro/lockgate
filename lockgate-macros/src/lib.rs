use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{Ident, LitStr, Token, braced, parse::Parse, parse::ParseStream};
use wit_parser::{InterfaceId, Resolve, WorldId, WorldItem};

#[proc_macro]
pub fn bindings(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as BindingsInput);
    match expand_bindings(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

#[proc_macro]
pub fn bindgen(input: TokenStream) -> TokenStream {
    let input = TokenStream2::from(input);
    let world = match find_string_option(&input, "world") {
        Some(world) => world,
        None => {
            return syn::Error::new_spanned(
                &input,
                "lockgate::bindgen! requires an explicit `world` string",
            )
            .into_compile_error()
            .into();
        }
    };
    let (world_name, exports, imports) = match load_world(&input, &world) {
        Ok(result) => result,
        Err(error) => return error.into_compile_error().into(),
    };
    let rust_name = match world_name.as_str() {
        "host" => "Host_".into(),
        name => name.to_upper_camel_case(),
    };
    let binding = format_ident!("{rust_name}");
    let lockgate = match crate_name("lockgate") {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            quote!(::#name)
        }
        Err(error) => {
            return syn::Error::new_spanned(
                &input,
                format!("failed to resolve the lockgate crate: {error}"),
            )
            .into_compile_error()
            .into();
        }
    };
    let export_metadata = exports.iter().map(|export| {
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
    let import_metadata = imports.iter().map(|import| {
        let interface = &import.interface;
        quote! {
            #lockgate::__private::BindingImport { interface: #interface }
        }
    });

    quote! {
        ::wasmtime::component::bindgen!(#input);

        const _: () = {
            impl #lockgate::__private::ComponentBinding for #binding {
                const WORLD: &'static str = #world;
                const EXPORTS: &'static [#lockgate::__private::BindingExport] =
                    &[#(#export_metadata),*];
                const IMPORTS: &'static [#lockgate::__private::BindingImport] =
                    &[#(#import_metadata),*];

                fn bind<H: Send + 'static>(
                    store: &mut #lockgate::__private::WasmtimeStore<
                        #lockgate::PluginStore<H>,
                    >,
                    instance: &#lockgate::__private::WasmtimeInstance,
                ) -> #lockgate::__private::AnyResult<Self> {
                    Ok(Self::new(&mut *store, instance)?)
                }
            }
        };
    }
    .into()
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
    imports: Vec<lockgate_schema::Import>,
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
        let imports = lockgate_schema::world_imports(&resolve, id)
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
            imports,
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

    let worlds = selected.iter().map(|selected| {
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
        let import_metadata = selected.imports.iter().map(|import| {
            let interface = &import.interface;
            quote! {
                #lockgate::__private::BindingImport { interface: #interface }
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
        let host_bounds = host_imports.values().map(|id| {
            let path = interface_module_path(&resolve, *id);
            quote! {
                #lockgate::HostContext<S>:
                    super::__lockgate_shared_imports::#(#path)::*::Host,
            }
        });
        let installers = host_imports.iter().enumerate().map(|(index, (_, id))| {
            let installer = format_ident!("__lockgate_install_host_import_{index}");
            let path = interface_module_path(&resolve, *id);
            quote! {
                fn #installer<S: Send + 'static>(
                    linker: &mut ::wasmtime::component::Linker<
                        #lockgate::PluginStore<S>,
                    >,
                ) -> #lockgate::__private::AnyResult<()>
                where
                    #lockgate::HostContext<S>:
                        super::__lockgate_shared_imports::#(#path)::*::Host,
                {
                    super::__lockgate_shared_imports::#(#path)::*::add_to_linker::<
                        _,
                        #lockgate::__private::HostContextData<S>,
                    >(linker, #lockgate::PluginStore::context_mut)?;
                    Ok(())
                }
            }
        });
        let installer_entries = host_imports.keys().enumerate().map(|(index, interface)| {
            let installer = format_ident!("__lockgate_install_host_import_{index}");
            quote! {
                #lockgate::__private::HostImportBinding {
                    interface: #interface,
                    install: #installer::<S>,
                }
            }
        });
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
        quote! {
            #[doc(hidden)]
            pub mod #module {
                ::wasmtime::component::bindgen!({
                    path: #path,
                    world: #world,
                    with: { #(#remappings),* },
                });

                const _: () = {
                    #(#installers)*

                    impl #lockgate::__private::ComponentBinding for #rust_name {
                        const WORLD: &'static str = #world;
                        const EXPORTS: &'static [#lockgate::__private::BindingExport] =
                            &[#(#export_metadata),*];
                        const IMPORTS: &'static [#lockgate::__private::BindingImport] =
                            &[#(#import_metadata),*];

                        fn bind<H: Send + 'static>(
                            store: &mut #lockgate::__private::WasmtimeStore<
                                #lockgate::PluginStore<H>,
                            >,
                            instance: &#lockgate::__private::WasmtimeInstance,
                        ) -> #lockgate::__private::AnyResult<Self> {
                            Ok(Self::new(&mut *store, instance)?)
                        }
                    }

                    impl<S: Send + 'static> #lockgate::__private::ApplicationBinding<S>
                        for #rust_name
                    where
                        #(#host_bounds)*
                    {
                        fn host_imports() -> ::std::vec::Vec<
                            #lockgate::__private::HostImportBinding<S>,
                        > {
                            ::std::vec![#(#installer_entries),*]
                        }
                    }
                };
            }

            pub use #module::#rust_name as #alias;
        }
    });

    Ok(quote! {
        #shared_bindings
        #(#shared_reexports)*
        #(#worlds)*
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

fn load_world(
    input: &TokenStream2,
    world: &LitStr,
) -> syn::Result<(
    String,
    Vec<lockgate_schema::Export>,
    Vec<lockgate_schema::Import>,
)> {
    let mut resolve = Resolve::new();
    resolve.all_features = true;
    let packages = if let Some(path) = find_string_option(input, "path") {
        let root = std::env::var("CARGO_MANIFEST_DIR").map_err(|error| {
            syn::Error::new_spanned(&path, format!("failed to locate package root: {error}"))
        })?;
        let path = std::path::Path::new(&root).join(path.value());
        let (package, _) = resolve.push_path(&path).map_err(|error| {
            syn::Error::new_spanned(
                world,
                format!("failed to parse {}: {error:#}", path.display()),
            )
        })?;
        vec![package]
    } else if let Some(inline) = find_string_option(input, "inline") {
        let package = resolve
            .push_str("lockgate-bindgen.wit", &inline.value())
            .map_err(|error| syn::Error::new_spanned(&inline, error.to_string()))?;
        vec![package]
    } else {
        return Err(syn::Error::new_spanned(
            input,
            "lockgate::bindgen! requires an explicit `path` or `inline` source",
        ));
    };
    let selected = resolve
        .select_world(&packages, Some(&world.value()))
        .map_err(|error| syn::Error::new_spanned(world, error.to_string()))?;
    let name = resolve.worlds[selected].name.clone();
    let exports = lockgate_schema::world_exports(&resolve, selected)
        .map_err(|error| syn::Error::new_spanned(world, error.to_string()))?;
    let imports = lockgate_schema::world_imports(&resolve, selected)
        .map_err(|error| syn::Error::new_spanned(world, error.to_string()))?;
    Ok((name, exports, imports))
}

fn find_string_option(tokens: &TokenStream2, option: &str) -> Option<LitStr> {
    let tokens = tokens.clone().into_iter().collect::<Vec<_>>();
    for window in tokens.windows(3) {
        let [
            TokenTree::Ident(name),
            TokenTree::Punct(colon),
            TokenTree::Literal(value),
        ] = window
        else {
            continue;
        };
        if name == option && colon.as_char() == ':' {
            return syn::parse_str(&value.to_string()).ok();
        }
    }
    tokens.into_iter().find_map(|token| match token {
        TokenTree::Group(group) => find_string_option(&group.stream(), option),
        _ => None,
    })
}
