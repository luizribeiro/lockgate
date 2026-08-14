use std::collections::BTreeSet;

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, Item, ItemTrait, LitStr, PathArguments, ReturnType, Token, Type,
    TypeImplTrait, braced, parse::Parse, parse::ParseStream,
};
use wasmtime_wit_bindgen::{FunctionConfig, FunctionFilter, FunctionFlags, Opts};
use wit_parser::{InterfaceId, Resolve, WorldItem};

/// Generates application-facing host-import traits and their linker adapters.
///
/// The macro takes `path`, `world`, `imports`, and `data` options. Each imported
/// WIT interface becomes a top-level Rust module with a `Host` trait; an
/// implementation may use `async fn` methods whose second parameter is
/// `HostCtx<'_, data>`.
///
/// The `imports` type is cloned once for each fresh plugin Store and once more
/// for each overlapping host call. Shared application state should therefore
/// live in explicitly shared fields such as `Arc<T>`.
#[proc_macro]
pub fn host_bindings(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as HostBindingsInput);
    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct HostBindingsInput {
    path: LitStr,
    world: LitStr,
    imports: Type,
    data: Type,
}

impl Parse for HostBindingsInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            let parsed = Self::parse_options(&content)?;
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after host bindings options"));
            }
            return Ok(parsed);
        }
        Self::parse_options(input)
    }
}

impl HostBindingsInput {
    fn parse_options(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut path = None;
        let mut world = None;
        let mut imports = None;
        let mut data = None;
        while !input.is_empty() {
            let option: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match option.to_string().as_str() {
                "path" => set_once(&mut path, input.parse()?, &option)?,
                "world" => set_once(&mut world, input.parse()?, &option)?,
                "imports" => set_once(&mut imports, input.parse()?, &option)?,
                "data" => set_once(&mut data, input.parse()?, &option)?,
                name => {
                    return Err(syn::Error::new(
                        option.span(),
                        format!("unsupported lockgate::host_bindings! option `{name}`"),
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            path: required(path, input, "path")?,
            world: required(world, input, "world")?,
            imports: required(imports, input, "imports")?,
            data: required(data, input, "data")?,
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &Ident) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            option.span(),
            format!("duplicate `{option}` option"),
        ));
    }
    Ok(())
}

fn required<T>(value: Option<T>, input: ParseStream<'_>, name: &str) -> syn::Result<T> {
    value.ok_or_else(|| {
        syn::Error::new(
            input.span(),
            format!("lockgate::host_bindings! requires a `{name}` option"),
        )
    })
}

struct ImportedInterface {
    path: Vec<Ident>,
    public_module: Ident,
    public_types: Vec<Ident>,
}

fn expand(input: HostBindingsInput) -> syn::Result<TokenStream2> {
    let lockgate = lockgate_path(&input.path)?;
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|error| {
        syn::Error::new_spanned(
            &input.path,
            format!("failed to locate package root: {error}"),
        )
    })?;
    let source = std::path::Path::new(&manifest_dir).join(input.path.value());
    let mut resolve = Resolve::new();
    resolve.all_features = true;
    let (package, _) = resolve.push_path(&source).map_err(|error| {
        syn::Error::new_spanned(
            &input.path,
            format!("failed to parse {}: {error:#}", source.display()),
        )
    })?;
    let world = resolve
        .select_world(&[package], Some(&input.world.value()))
        .map_err(|error| syn::Error::new_spanned(&input.world, error.to_string()))?;

    let mut module_names = BTreeSet::new();
    let mut interfaces = Vec::new();
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            return Err(syn::Error::new_spanned(
                &input.world,
                "host bindings require imports to be named WIT interfaces",
            ));
        };
        let interface = &resolve.interfaces[*id];
        let Some(name) = interface.name.as_deref() else {
            return Err(syn::Error::new_spanned(
                &input.world,
                "host bindings do not support unnamed imported interfaces",
            ));
        };
        let public_module = format_ident!("{}", rust_ident(name));
        if !module_names.insert(public_module.to_string()) {
            return Err(syn::Error::new_spanned(
                &input.world,
                format!("two imported interfaces map to module `{public_module}`"),
            ));
        }
        let public_types = interface
            .types
            .keys()
            .map(|name| format_ident!("{}", rust_type_ident(name)))
            .collect();
        interfaces.push(ImportedInterface {
            path: interface_module_path(&resolve, *id),
            public_module,
            public_types,
        });
    }

    let mut imports_config = FunctionConfig::new();
    imports_config.push(
        FunctionFilter::Default,
        FunctionFlags::ASYNC | FunctionFlags::STORE,
    );
    let mut options = Opts {
        imports: imports_config,
        ..Default::default()
    };
    options.wasmtime_crate = Some(format!("{lockgate}::__private::wasmtime"));
    let generated = options
        .generate(&mut resolve, world)
        .map_err(|error| syn::Error::new_spanned(&input.world, error.to_string()))?;
    let mut generated = syn::parse_file(&generated).map_err(|error| {
        syn::Error::new_spanned(
            &input.world,
            format!("failed to read generated Wasmtime bindings: {error}"),
        )
    })?;

    let imports = &input.imports;
    let data = &input.data;
    for interface in &interfaces {
        let items =
            nested_module_items_mut(&mut generated.items, &interface.path).ok_or_else(|| {
                syn::Error::new_spanned(
                    &input.world,
                    format!(
                        "could not locate generated host interface `{}`",
                        interface.public_module
                    ),
                )
            })?;
        let host = items
            .iter()
            .find_map(|item| match item {
                Item::Trait(item) if item.ident == "HostWithStore" => Some(item.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    &input.world,
                    format!(
                        "generated interface `{}` has no host function trait",
                        interface.public_module
                    ),
                )
            })?;
        items.extend(adapter_items(&host, &lockgate)?);
    }

    let generated_items = generated.items;
    let public_modules = interfaces.iter().map(|interface| {
        let module = &interface.public_module;
        let path = &interface.path;
        let raw = quote!(super::__lockgate_host_bindings::#(#path)::*);
        let type_reexports = interface
            .public_types
            .iter()
            .map(|ty| quote!(pub use #raw::#ty;));
        quote! {
            pub mod #module {
                pub use #raw::__LockgateHost as Host;
                #(#type_reexports)*
            }
        }
    });
    let host_bounds = interfaces.iter().map(|interface| {
        let module = &interface.public_module;
        quote!(#imports: #module::Host,)
    });
    let registrations = interfaces.iter().map(|interface| {
        let path = &interface.path;
        quote! {
            __lockgate_host_bindings::#(#path)::*::__lockgate_register(linker)?;
        }
    });

    Ok(quote! {
        #[doc(hidden)]
        pub mod __lockgate_host_bindings {
            use super::*;
            type __LockgateImports = #imports;
            type __LockgateData = #data;
            #(#generated_items)*
        }

        #(#public_modules)*

        impl #lockgate::HostImports<#data> for #imports
        where
            #(#host_bounds)*
        {
            fn add_to_linker(
                &self,
                linker: &mut #lockgate::__private::wasmtime::component::Linker<
                    #lockgate::__private::StoreCtx<#data>,
                >,
            ) -> #lockgate::__private::wasmtime::Result<()> {
                #(#registrations)*
                Ok(())
            }
        }
    })
}

fn adapter_items(host: &ItemTrait, lockgate: &TokenStream2) -> syn::Result<Vec<Item>> {
    let imports = quote!(super::super::super::__LockgateImports);
    let data = quote!(super::super::super::__LockgateData);
    let mut public_methods = Vec::new();
    let mut adapter_methods = Vec::new();
    for item in &host.items {
        let syn::TraitItem::Fn(method) = item else {
            continue;
        };
        let mut inputs = method.sig.inputs.iter();
        let Some(store) = inputs.next() else {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "generated host method is missing its Store accessor",
            ));
        };
        let store_is_accessor = match store {
            FnArg::Typed(store) => matches!(store.ty.as_ref(), Type::Reference(_)),
            FnArg::Receiver(_) => false,
        };
        let inputs = inputs.cloned().collect::<Vec<_>>();
        let arguments = inputs
            .iter()
            .map(|input| match input {
                FnArg::Typed(input) => match input.pat.as_ref() {
                    syn::Pat::Ident(ident) => Ok(ident.ident.clone()),
                    pattern => Err(syn::Error::new_spanned(
                        pattern,
                        "generated host parameter must use an identifier",
                    )),
                },
                FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(
                    receiver,
                    "generated host parameter unexpectedly uses a receiver",
                )),
            })
            .collect::<syn::Result<Vec<_>>>()?;
        let result = future_output(&method.sig.output)?;
        let name = &method.sig.ident;
        public_methods.push(quote! {
            fn #name(
                &mut self,
                cx: #lockgate::HostCtx<'_, #data>,
                #(#inputs),*
            ) -> impl ::core::future::Future<Output = #result> + Send;
        });
        let (host_input, host_parts) = if store_is_accessor {
            (
                quote! {
                    host: &#lockgate::__private::wasmtime::component::Accessor<
                        #lockgate::__private::StoreCtx<#data>,
                        Self,
                    >
                },
                quote! {
                    host.with(|mut access| {
                        access
                            .data_mut()
                            .host_parts::<#imports, #lockgate::PluginHandle>()
                    })
                },
            )
        } else {
            (
                quote! {
                    mut host: #lockgate::__private::wasmtime::component::Access<
                        #lockgate::__private::StoreCtx<#data>,
                        Self,
                    >
                },
                quote! {
                    host.data_mut()
                        .host_parts::<#imports, #lockgate::PluginHandle>()
                },
            )
        };
        adapter_methods.push(quote! {
            fn #name(
                #host_input,
                #(#inputs),*
            ) -> impl ::core::future::Future<Output = #result> + Send {
                async move {
                    let (mut imports, data, plugin, jobs) = #host_parts;
                    let cx = #lockgate::HostCtx::new(data.as_ref(), plugin.as_ref(), jobs);
                    <#imports as __LockgateHost>::#name(
                        &mut imports,
                        cx,
                        #(#arguments),*
                    ).await
                }
            }
        });
    }

    syn::parse2::<syn::File>(quote! {
        #[doc = "Application implementation of this imported WIT interface."]
        pub trait __LockgateHost: Send {
            #(#public_methods)*
        }

        struct __LockgateAdapter;

        impl #lockgate::__private::wasmtime::component::HasData for __LockgateAdapter {
            type Data<'a> = ();
        }

        impl Host for () {}

        impl HostWithStore<#lockgate::__private::StoreCtx<#data>> for __LockgateAdapter {
            #(#adapter_methods)*
        }

        pub fn __lockgate_register(
            linker: &mut #lockgate::__private::wasmtime::component::Linker<
                #lockgate::__private::StoreCtx<#data>,
            >,
        ) -> #lockgate::__private::wasmtime::Result<()> {
            add_to_linker::<_, __LockgateAdapter>(linker, |_| ())
        }
    })
    .map(|file| file.items)
}

fn future_output(output: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "generated host method is missing its Future return type",
        ));
    };
    let Type::ImplTrait(TypeImplTrait { bounds, .. }) = ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            ty,
            "generated host method return type is not a Future",
        ));
    };
    for bound in bounds {
        let syn::TypeParamBound::Trait(bound) = bound else {
            continue;
        };
        let Some(segment) = bound.path.segments.last() else {
            continue;
        };
        if segment.ident != "Future" {
            continue;
        }
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            continue;
        };
        for argument in &arguments.args {
            if let GenericArgument::AssocType(output) = argument
                && output.ident == "Output"
            {
                return Ok(output.ty.clone());
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "generated host Future has no Output type",
    ))
}

fn nested_module_items_mut<'a>(
    items: &'a mut Vec<Item>,
    path: &[Ident],
) -> Option<&'a mut Vec<Item>> {
    let Some((segment, rest)) = path.split_first() else {
        return Some(items);
    };
    let module = items.iter_mut().find_map(|item| match item {
        Item::Mod(module) if module.ident == *segment => Some(module),
        _ => None,
    })?;
    let (_, items) = module.content.as_mut()?;
    nested_module_items_mut(items, rest)
}

fn interface_module_path(resolve: &Resolve, id: InterfaceId) -> Vec<Ident> {
    let interface = &resolve.interfaces[id];
    let package_id = interface.package.expect("named imports have a package");
    let package = &resolve.packages[package_id];
    let mut package_name = package.name.name.to_snake_case();
    let versions = resolve
        .packages
        .iter()
        .filter(|(_, candidate)| {
            candidate.name.namespace == package.name.namespace
                && candidate.name.name == package.name.name
        })
        .count();
    if versions > 1
        && let Some(version) = &package.name.version
    {
        package_name.push_str(
            &version
                .to_string()
                .replace(['.', '-', '+'], "_")
                .to_snake_case(),
        );
    }
    [
        package.name.namespace.as_str(),
        package_name.as_str(),
        interface.name.as_deref().expect("named interface"),
    ]
    .into_iter()
    .map(|name| format_ident!("{}", rust_ident(name)))
    .collect()
}

fn rust_ident(name: &str) -> String {
    let name = name.to_snake_case();
    if matches!(
        name.as_str(),
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "gen"
    ) {
        format!("{name}_")
    } else {
        name
    }
}

fn rust_type_ident(name: &str) -> String {
    let name = name.to_upper_camel_case();
    if matches!(name.as_str(), "Self" | "Result" | "Option") {
        format!("{name}_")
    } else {
        name
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
