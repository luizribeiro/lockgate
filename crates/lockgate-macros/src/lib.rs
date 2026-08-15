use std::collections::{BTreeMap, BTreeSet};

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, Item, ItemTrait, LitStr, PathArguments, ReturnType, Token,
    Type as SynType, TypeImplTrait, braced, parse::Parse, parse::ParseStream,
};
use wasmtime_wit_bindgen::{FunctionConfig, FunctionFilter, FunctionFlags, Opts};
use wit_parser::{InterfaceId, Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldItem};

/// Generates typed application bindings for a WIT world.
///
/// The macro takes `path` and `world` options. Worlds with imported interfaces
/// also provide `imports` and `data` as a pair; `data` names the call-context
/// type, and export-only worlds omit both.
/// Each imported WIT interface becomes a top-level Rust module with a `Host`
/// trait; an implementation may use `async fn` methods whose second parameter
/// is `HostCtx<'_, data>`.
///
/// Each named exported interface becomes a top-level module containing its WIT
/// value types plus `Role`, `Client`, and `HostExt`. Client methods take an
/// `InvocationCtx` before their WIT arguments and invoke the plugin through
/// Lockgate's value-only call boundary.
/// Types reused from another interface are available when that defining
/// interface is also exported by the selected world; other cross-interface
/// type references are rejected during macro expansion.
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
    imports: Option<HostImportsConfig>,
}

struct HostImportsConfig {
    imports: SynType,
    data: SynType,
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

        let imports = match (imports, data) {
            (Some(imports), Some(data)) => Some(HostImportsConfig { imports, data }),
            (None, None) => None,
            _ => {
                return Err(input.error(
                    "lockgate::host_bindings! options `imports` and `data` must be specified together",
                ));
            }
        };

        Ok(Self {
            path: required(path, input, "path")?,
            world: required(world, input, "world")?,
            imports,
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

fn export_module(
    resolve: &Resolve,
    exported: &ExportedInterface,
    exported_modules: &BTreeMap<InterfaceId, Ident>,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let interface = &resolve.interfaces[exported.id];
    let module = &exported.public_module;
    let extension_method = module;
    let interface_name = &exported.interface_name;
    let type_names = interface
        .types
        .iter()
        .map(|(name, id)| (*id, format_ident!("{}", rust_type_ident(name))))
        .collect::<BTreeMap<_, _>>();
    let types = ExportTypeContext {
        resolve,
        interface: exported.id,
        type_names,
        exported_modules,
    };
    let type_definitions = interface
        .types
        .iter()
        .map(|(name, id)| {
            public_type_definition(&types, *id, format_ident!("{}", rust_type_ident(name)))
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let methods = interface
        .functions
        .values()
        .map(|function| client_method(&types, function, lockgate))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        pub mod #module {
            #(#type_definitions)*

            /// Role marker for this exported WIT interface.
            pub struct Role;

            /// Typed client for one admitted plugin's exported WIT interface.
            pub struct Client<'a, S: #lockgate::CallContext> {
                invocation: #lockgate::RoleInvocation<'a, S>,
            }

            impl #lockgate::Role for Role {
                const INTERFACE: &'static str = #interface_name;

                type Client<'a, S>
                    = Client<'a, S>
                where
                    S: #lockgate::CallContext;

                fn client<'a, S>(
                    invocation: #lockgate::RoleInvocation<'a, S>,
                ) -> Self::Client<'a, S>
                where
                    S: #lockgate::CallContext,
                {
                    Client { invocation }
                }
            }

            impl<S: #lockgate::CallContext> Client<'_, S> {
                #(#methods)*
            }

            /// Readable cast extension for this exported WIT interface.
            pub trait HostExt<S: #lockgate::CallContext> {
                fn #extension_method(
                    &self,
                    plugin: &#lockgate::PluginHandle,
                ) -> Result<Client<'_, S>, #lockgate::RoleError>;
            }

            impl<S: #lockgate::CallContext> HostExt<S> for #lockgate::Host<S> {
                fn #extension_method(
                    &self,
                    plugin: &#lockgate::PluginHandle,
                ) -> Result<Client<'_, S>, #lockgate::RoleError> {
                    self.client::<Role>(plugin)
                }
            }
        }
    })
}

fn public_type_definition(
    types: &ExportTypeContext<'_>,
    id: TypeId,
    name: Ident,
) -> syn::Result<TokenStream2> {
    if !types.is_local(id) {
        let ty = types.required_named_type(id, types.resolve.types[id].kind.as_str())?;
        return Ok(quote!(pub type #name = #ty;));
    }

    Ok(match &types.resolve.types[id].kind {
        TypeDefKind::Record(record) => {
            let fields = record
                .fields
                .iter()
                .map(|field| {
                    let field_name = format_ident!("{}", rust_ident(&field.name));
                    let ty = rust_type(types, field.ty, false)?;
                    Ok(quote!(pub #field_name: #ty))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote! {
                #[derive(Clone, Debug, PartialEq)]
                pub struct #name { #(#fields,)* }
            }
        }
        TypeDefKind::Variant(variant) => {
            let cases = variant
                .cases
                .iter()
                .map(|case| {
                    let case_name = format_ident!("{}", rust_type_ident(&case.name));
                    match case.ty {
                        Some(ty) => {
                            let ty = rust_type(types, ty, false)?;
                            Ok(quote!(#case_name(#ty)))
                        }
                        None => Ok(quote!(#case_name)),
                    }
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote! {
                #[derive(Clone, Debug, PartialEq)]
                pub enum #name { #(#cases,)* }
            }
        }
        TypeDefKind::Enum(enum_) => {
            let cases = enum_
                .cases
                .iter()
                .map(|case| format_ident!("{}", rust_type_ident(&case.name)));
            quote! {
                #[derive(Clone, Copy, Debug, PartialEq, Eq)]
                pub enum #name { #(#cases,)* }
            }
        }
        TypeDefKind::Flags(flags) => flags_definition(&name, flags),
        TypeDefKind::Tuple(_)
        | TypeDefKind::Option(_)
        | TypeDefKind::Result(_)
        | TypeDefKind::List(_)
        | TypeDefKind::Map(_, _)
        | TypeDefKind::FixedLengthList(_, _)
        | TypeDefKind::Type(_) => {
            let ty = rust_anonymous_type(types, id)?;
            quote!(pub type #name = #ty;)
        }
        TypeDefKind::Resource => return Err(unsupported_codegen_type("resource")),
        TypeDefKind::Handle(_) => return Err(unsupported_codegen_type("resource handle")),
        TypeDefKind::Future(_) => return Err(unsupported_codegen_type("future")),
        TypeDefKind::Stream(_) => return Err(unsupported_codegen_type("stream")),
        TypeDefKind::Unknown => return Err(unsupported_codegen_type("unknown")),
    })
}

fn flags_definition(name: &Ident, flags: &wit_parser::Flags) -> TokenStream2 {
    let word_count = flags.flags.len().div_ceil(32);
    let constants = flags.flags.iter().enumerate().map(|(index, flag)| {
        let constant = format_ident!("{}", flag.name.to_shouty_snake_case());
        let word = index / 32;
        let bit = index % 32;
        let words =
            (0..word_count).map(|candidate| if candidate == word { 1u32 << bit } else { 0 });
        quote!(pub const #constant: Self = Self([#(#words),*]);)
    });
    let indices = (0..word_count).map(syn::Index::from).collect::<Vec<_>>();
    let contains = if indices.is_empty() {
        quote!(true)
    } else {
        quote!(#((self.0[#indices] & other.0[#indices]) == other.0[#indices])&&*)
    };

    quote! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct #name([u32; #word_count]);

        impl #name {
            pub const fn empty() -> Self {
                Self([0; #word_count])
            }

            #(#constants)*

            pub fn contains(self, other: Self) -> bool {
                #contains
            }
        }

        impl std::ops::BitOr for #name {
            type Output = Self;

            fn bitor(self, other: Self) -> Self {
                Self([#(self.0[#indices] | other.0[#indices]),*])
            }
        }

        impl std::ops::BitOrAssign for #name {
            fn bitor_assign(&mut self, other: Self) {
                *self = *self | other;
            }
        }
    }
}

fn client_method(
    types: &ExportTypeContext<'_>,
    function: &wit_parser::Function,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let method = format_ident!("{}", rust_ident(&function.name));
    let function_name = &function.name;
    let mut parameters = Vec::new();
    let mut arguments = Vec::new();
    for parameter in &function.params {
        let name = format_ident!("{}", rust_ident(&parameter.name));
        let ty = rust_type(types, parameter.ty, true)?;
        let value = lower_value(types, parameter.ty, quote!(#name), true, lockgate)?;
        parameters.push(quote!(#name: #ty));
        arguments.push(value);
    }
    let result_type = function
        .result
        .map(|ty| rust_type(types, ty, false))
        .transpose()?
        .unwrap_or_else(|| quote!(()));
    let lifted = match function.result {
        Some(ty) => {
            let value = lift_value(types, ty, quote!(value), lockgate)?;
            let message =
                format!("function `{function_name}` returned a value with an unexpected shape");
            quote! {
                match results.as_slice() {
                    [value] => #value,
                    _ => Err(#lockgate::CallError::shape(#message)),
                }
            }
        }
        None => {
            let message = format!("function `{function_name}` unexpectedly returned values");
            quote! {
                if results.is_empty() {
                    Ok(())
                } else {
                    Err(#lockgate::CallError::shape(#message))
                }
            }
        }
    };

    Ok(quote! {
        pub async fn #method(
            &self,
            ctx: #lockgate::InvocationCtx<S>,
            #(#parameters),*
        ) -> Result<#result_type, #lockgate::CallError> {
            let results = self
                .invocation
                .invoke(#function_name, &[#(#arguments),*], ctx)
                .await?;
            #lifted
        }
    })
}

fn rust_type(
    types: &ExportTypeContext<'_>,
    ty: Type,
    borrow_string: bool,
) -> syn::Result<TokenStream2> {
    Ok(match ty {
        Type::Bool => quote!(bool),
        Type::U8 => quote!(u8),
        Type::U16 => quote!(u16),
        Type::U32 => quote!(u32),
        Type::U64 => quote!(u64),
        Type::S8 => quote!(i8),
        Type::S16 => quote!(i16),
        Type::S32 => quote!(i32),
        Type::S64 => quote!(i64),
        Type::F32 => quote!(f32),
        Type::F64 => quote!(f64),
        Type::Char => quote!(char),
        Type::String if borrow_string => quote!(&str),
        Type::String => quote!(String),
        Type::ErrorContext => return Err(unsupported_codegen_type("error-context")),
        Type::Id(id) => {
            if let Some(name) = types.named_type(id)? {
                quote!(#name)
            } else {
                rust_anonymous_type(types, id)?
            }
        }
    })
}

fn rust_anonymous_type(types: &ExportTypeContext<'_>, id: TypeId) -> syn::Result<TokenStream2> {
    Ok(match &types.resolve.types[id].kind {
        TypeDefKind::Tuple(tuple) => {
            let types = tuple
                .types
                .iter()
                .map(|ty| rust_type(types, *ty, false))
                .collect::<syn::Result<Vec<_>>>()?;
            quote!((#(#types,)*))
        }
        TypeDefKind::Option(ty) => {
            let ty = rust_type(types, *ty, false)?;
            quote!(Option<#ty>)
        }
        TypeDefKind::Result(result) => {
            let ok = optional_rust_type(types, result.ok)?;
            let err = optional_rust_type(types, result.err)?;
            quote!(Result<#ok, #err>)
        }
        TypeDefKind::List(ty) => {
            let ty = rust_type(types, *ty, false)?;
            quote!(Vec<#ty>)
        }
        TypeDefKind::Map(key, value) => {
            let key = rust_type(types, *key, false)?;
            let value = rust_type(types, *value, false)?;
            quote!(std::collections::HashMap<#key, #value>)
        }
        TypeDefKind::FixedLengthList(ty, size) => {
            let ty = rust_type(types, *ty, false)?;
            let size = *size as usize;
            quote!([#ty; #size])
        }
        TypeDefKind::Type(ty) => rust_type(types, *ty, false)?,
        TypeDefKind::Record(_) => return Err(unnamed_codegen_type("record")),
        TypeDefKind::Flags(_) => return Err(unnamed_codegen_type("flags")),
        TypeDefKind::Variant(_) => return Err(unnamed_codegen_type("variant")),
        TypeDefKind::Enum(_) => return Err(unnamed_codegen_type("enum")),
        TypeDefKind::Resource => return Err(unsupported_codegen_type("resource")),
        TypeDefKind::Handle(_) => return Err(unsupported_codegen_type("resource handle")),
        TypeDefKind::Future(_) => return Err(unsupported_codegen_type("future")),
        TypeDefKind::Stream(_) => return Err(unsupported_codegen_type("stream")),
        TypeDefKind::Unknown => return Err(unsupported_codegen_type("unknown")),
    })
}

fn optional_rust_type(
    types: &ExportTypeContext<'_>,
    ty: Option<Type>,
) -> syn::Result<TokenStream2> {
    ty.map(|ty| rust_type(types, ty, false))
        .transpose()
        .map(|ty| ty.unwrap_or_else(|| quote!(())))
}

fn lower_value(
    types: &ExportTypeContext<'_>,
    ty: Type,
    value: TokenStream2,
    borrowed_string: bool,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    Ok(match ty {
        Type::Bool => quote!(#lockgate::Value::Bool(#value)),
        Type::U8 => quote!(#lockgate::Value::U8(#value)),
        Type::U16 => quote!(#lockgate::Value::U16(#value)),
        Type::U32 => quote!(#lockgate::Value::U32(#value)),
        Type::U64 => quote!(#lockgate::Value::U64(#value)),
        Type::S8 => quote!(#lockgate::Value::S8(#value)),
        Type::S16 => quote!(#lockgate::Value::S16(#value)),
        Type::S32 => quote!(#lockgate::Value::S32(#value)),
        Type::S64 => quote!(#lockgate::Value::S64(#value)),
        Type::F32 => quote!(#lockgate::Value::Float32(#value)),
        Type::F64 => quote!(#lockgate::Value::Float64(#value)),
        Type::Char => quote!(#lockgate::Value::Char(#value)),
        Type::String if borrowed_string => quote!(#lockgate::Value::String(#value.to_owned())),
        Type::String => quote!(#lockgate::Value::String(#value)),
        Type::ErrorContext => return Err(unsupported_codegen_type("error-context")),
        Type::Id(id) => lower_type_id(types, id, value, lockgate)?,
    })
}

fn lower_type_id(
    types: &ExportTypeContext<'_>,
    id: TypeId,
    value: TokenStream2,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    Ok(match &types.resolve.types[id].kind {
        TypeDefKind::Record(record) => {
            let fields = record
                .fields
                .iter()
                .map(|field| {
                    let name = &field.name;
                    let member = format_ident!("{}", rust_ident(name));
                    let value =
                        lower_value(types, field.ty, quote!(#value.#member), false, lockgate)?;
                    Ok(quote!((#name.to_owned(), #value)))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(#lockgate::Value::Record(vec![#(#fields),*]))
        }
        TypeDefKind::Tuple(tuple) => {
            let values = tuple
                .types
                .iter()
                .enumerate()
                .map(|(index, ty)| {
                    let index = syn::Index::from(index);
                    lower_value(types, *ty, quote!(#value.#index), false, lockgate)
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(#lockgate::Value::Tuple(vec![#(#values),*]))
        }
        TypeDefKind::Variant(variant) => {
            let ty = types.required_named_type(id, "variant")?;
            let cases = variant
                .cases
                .iter()
                .map(|case| {
                    let name = &case.name;
                    let case_name = format_ident!("{}", rust_type_ident(name));
                    Ok(match case.ty {
                        Some(payload) => {
                            let payload =
                                lower_value(types, payload, quote!(payload), false, lockgate)?;
                            quote!(#ty::#case_name(payload) => #lockgate::Value::Variant(
                                #name.to_owned(), Some(Box::new(#payload)),
                            ))
                        }
                        None => quote!(#ty::#case_name => #lockgate::Value::Variant(
                            #name.to_owned(), None,
                        )),
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(match #value { #(#cases),* })
        }
        TypeDefKind::Enum(enum_) => {
            let ty = types.required_named_type(id, "enum")?;
            let cases = enum_.cases.iter().map(|case| {
                let name = &case.name;
                let case_name = format_ident!("{}", rust_type_ident(name));
                quote!(#ty::#case_name => #lockgate::Value::Enum(#name.to_owned()))
            });
            quote!(match #value { #(#cases),* })
        }
        TypeDefKind::Option(ty) => {
            let lowered = lower_value(types, *ty, quote!(value), false, lockgate)?;
            quote!(#lockgate::Value::Option(
                #value.map(|value| Box::new(#lowered))
            ))
        }
        TypeDefKind::Result(result) => {
            let ok = lower_optional_value(types, result.ok, quote!(value), lockgate)?;
            let err = lower_optional_value(types, result.err, quote!(value), lockgate)?;
            quote!(#lockgate::Value::Result(match #value {
                Ok(value) => Ok(#ok),
                Err(value) => Err(#err),
            }))
        }
        TypeDefKind::List(ty) => {
            let element = lower_value(types, *ty, quote!(value), false, lockgate)?;
            quote!(#lockgate::Value::List(
                #value.into_iter().map(|value| #element).collect()
            ))
        }
        TypeDefKind::Map(key, item) => {
            let key = lower_value(types, *key, quote!(key), false, lockgate)?;
            let item = lower_value(types, *item, quote!(value), false, lockgate)?;
            quote!(#lockgate::Value::Map(
                #value
                    .into_iter()
                    .map(|(key, value)| (#key, #item))
                    .collect()
            ))
        }
        TypeDefKind::FixedLengthList(ty, _) => {
            let element = lower_value(types, *ty, quote!(value), false, lockgate)?;
            quote!(#lockgate::Value::FixedLengthList(
                #value.into_iter().map(|value| #element).collect()
            ))
        }
        TypeDefKind::Flags(flags) => {
            let ty = types.required_named_type(id, "flags")?;
            let pushes = flags.flags.iter().map(|flag| {
                let name = &flag.name;
                let constant = format_ident!("{}", name.to_shouty_snake_case());
                quote! {
                    if #value.contains(#ty::#constant) {
                        names.push(#name.to_owned());
                    }
                }
            });
            quote!({
                let mut names = Vec::new();
                #(#pushes)*
                #lockgate::Value::Flags(names)
            })
        }
        TypeDefKind::Type(ty) => lower_value(types, *ty, value, false, lockgate)?,
        TypeDefKind::Resource => return Err(unsupported_codegen_type("resource")),
        TypeDefKind::Handle(_) => return Err(unsupported_codegen_type("resource handle")),
        TypeDefKind::Future(_) => return Err(unsupported_codegen_type("future")),
        TypeDefKind::Stream(_) => return Err(unsupported_codegen_type("stream")),
        TypeDefKind::Unknown => return Err(unsupported_codegen_type("unknown")),
    })
}

fn lower_optional_value(
    types: &ExportTypeContext<'_>,
    ty: Option<Type>,
    value: TokenStream2,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    match ty {
        Some(ty) => {
            let value = lower_value(types, ty, value, false, lockgate)?;
            Ok(quote!(Some(Box::new(#value))))
        }
        None => Ok(quote!(None)),
    }
}

fn lift_value(
    types: &ExportTypeContext<'_>,
    ty: Type,
    value: TokenStream2,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let mismatch = |expected: &str| {
        let message = format!("expected {expected}");
        quote!(Err(#lockgate::CallError::shape(#message)))
    };
    Ok(match ty {
        Type::Bool => {
            let error = mismatch("a bool");
            quote!(match #value { #lockgate::Value::Bool(value) => Ok(*value), _ => #error })
        }
        Type::U8 => {
            let error = mismatch("a u8");
            quote!(match #value { #lockgate::Value::U8(value) => Ok(*value), _ => #error })
        }
        Type::U16 => {
            let error = mismatch("a u16");
            quote!(match #value { #lockgate::Value::U16(value) => Ok(*value), _ => #error })
        }
        Type::U32 => {
            let error = mismatch("a u32");
            quote!(match #value { #lockgate::Value::U32(value) => Ok(*value), _ => #error })
        }
        Type::U64 => {
            let error = mismatch("a u64");
            quote!(match #value { #lockgate::Value::U64(value) => Ok(*value), _ => #error })
        }
        Type::S8 => {
            let error = mismatch("an s8");
            quote!(match #value { #lockgate::Value::S8(value) => Ok(*value), _ => #error })
        }
        Type::S16 => {
            let error = mismatch("an s16");
            quote!(match #value { #lockgate::Value::S16(value) => Ok(*value), _ => #error })
        }
        Type::S32 => {
            let error = mismatch("an s32");
            quote!(match #value { #lockgate::Value::S32(value) => Ok(*value), _ => #error })
        }
        Type::S64 => {
            let error = mismatch("an s64");
            quote!(match #value { #lockgate::Value::S64(value) => Ok(*value), _ => #error })
        }
        Type::F32 => {
            let error = mismatch("an f32");
            quote!(match #value { #lockgate::Value::Float32(value) => Ok(*value), _ => #error })
        }
        Type::F64 => {
            let error = mismatch("an f64");
            quote!(match #value { #lockgate::Value::Float64(value) => Ok(*value), _ => #error })
        }
        Type::Char => {
            let error = mismatch("a char");
            quote!(match #value { #lockgate::Value::Char(value) => Ok(*value), _ => #error })
        }
        Type::String => {
            let error = mismatch("a string");
            quote!(match #value {
                #lockgate::Value::String(value) => Ok(value.clone()),
                _ => #error,
            })
        }
        Type::ErrorContext => return Err(unsupported_codegen_type("error-context")),
        Type::Id(id) => lift_type_id(types, id, value, lockgate)?,
    })
}

fn lift_type_id(
    types: &ExportTypeContext<'_>,
    id: TypeId,
    value: TokenStream2,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let expected = types.resolve.types[id]
        .name
        .as_deref()
        .unwrap_or_else(|| types.resolve.types[id].kind.as_str());
    let message = format!("expected {expected}");
    let mismatch = quote!(Err(#lockgate::CallError::shape(#message)));
    Ok(match &types.resolve.types[id].kind {
        TypeDefKind::Record(record) => {
            let ty = types.required_named_type(id, "record")?;
            let names = (0..record.fields.len())
                .map(|index| format_ident!("field_name_{index}"))
                .collect::<Vec<_>>();
            let values = (0..record.fields.len())
                .map(|index| format_ident!("field_value_{index}"))
                .collect::<Vec<_>>();
            let bindings = names
                .iter()
                .zip(&values)
                .map(|(name, value)| quote!((#name, #value)))
                .collect::<Vec<_>>();
            let guards = names.iter().zip(&record.fields).map(|(binding, field)| {
                let name = &field.name;
                quote!(#binding == #name)
            });
            let fields = record
                .fields
                .iter()
                .zip(&values)
                .map(|(field, value)| {
                    let member = format_ident!("{}", rust_ident(&field.name));
                    let lifted = lift_value(types, field.ty, quote!(#value), lockgate)?;
                    Ok(quote!(#member: (#lifted)?))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            let guard = if record.fields.is_empty() {
                quote!(true)
            } else {
                quote!(#(#guards)&&*)
            };
            quote!(match #value {
                #lockgate::Value::Record(fields) => match fields.as_slice() {
                    [#(#bindings),*] if #guard => Ok(#ty { #(#fields),* }),
                    _ => #mismatch,
                },
                _ => #mismatch,
            })
        }
        TypeDefKind::Tuple(tuple) => {
            let values = (0..tuple.types.len())
                .map(|index| format_ident!("item_{index}"))
                .collect::<Vec<_>>();
            let items = tuple
                .types
                .iter()
                .zip(&values)
                .map(|(ty, value)| {
                    let lifted = lift_value(types, *ty, quote!(#value), lockgate)?;
                    Ok(quote!((#lifted)?))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(match #value {
                #lockgate::Value::Tuple(values) => match values.as_slice() {
                    [#(#values),*] => Ok((#(#items,)*)),
                    _ => #mismatch,
                },
                _ => #mismatch,
            })
        }
        TypeDefKind::Variant(variant) => {
            let ty = types.required_named_type(id, "variant")?;
            let cases = variant
                .cases
                .iter()
                .map(|case| {
                    let name = &case.name;
                    let case_name = format_ident!("{}", rust_type_ident(name));
                    Ok(match case.ty {
                        Some(payload) => {
                            let lifted = lift_value(types, payload, quote!(payload), lockgate)?;
                            quote!((#name, Some(payload)) => Ok(#ty::#case_name((#lifted)?)))
                        }
                        None => quote!((#name, None) => Ok(#ty::#case_name)),
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(match #value {
                #lockgate::Value::Variant(case, payload) => {
                    match (case.as_str(), payload.as_deref()) {
                        #(#cases,)*
                        _ => #mismatch,
                    }
                }
                _ => #mismatch,
            })
        }
        TypeDefKind::Enum(enum_) => {
            let ty = types.required_named_type(id, "enum")?;
            let cases = enum_.cases.iter().map(|case| {
                let name = &case.name;
                let case_name = format_ident!("{}", rust_type_ident(name));
                quote!(#name => Ok(#ty::#case_name))
            });
            quote!(match #value {
                #lockgate::Value::Enum(case) => match case.as_str() {
                    #(#cases,)*
                    _ => #mismatch,
                },
                _ => #mismatch,
            })
        }
        TypeDefKind::Option(ty) => {
            let lifted = lift_value(types, *ty, quote!(value.as_ref()), lockgate)?;
            quote!(match #value {
                #lockgate::Value::Option(None) => Ok(None),
                #lockgate::Value::Option(Some(value)) => Ok(Some((#lifted)?)),
                _ => #mismatch,
            })
        }
        TypeDefKind::Result(result) => {
            let ok = lift_optional_value(types, result.ok, quote!(value.as_ref()), lockgate)?;
            let err = lift_optional_value(types, result.err, quote!(value.as_ref()), lockgate)?;
            let ok_pattern = if result.ok.is_some() {
                quote!(Some(value))
            } else {
                quote!(None)
            };
            let err_pattern = if result.err.is_some() {
                quote!(Some(value))
            } else {
                quote!(None)
            };
            quote!(match #value {
                #lockgate::Value::Result(Ok(#ok_pattern)) => Ok(Ok(#ok)),
                #lockgate::Value::Result(Err(#err_pattern)) => Ok(Err(#err)),
                _ => #mismatch,
            })
        }
        TypeDefKind::List(ty) => {
            let lifted = lift_value(types, *ty, quote!(value), lockgate)?;
            quote!(match #value {
                #lockgate::Value::List(values) => values
                    .iter()
                    .map(|value| #lifted)
                    .collect::<Result<Vec<_>, #lockgate::CallError>>(),
                _ => #mismatch,
            })
        }
        TypeDefKind::Map(key, item) => {
            let key = lift_value(types, *key, quote!(key), lockgate)?;
            let item = lift_value(types, *item, quote!(value), lockgate)?;
            quote!(match #value {
                #lockgate::Value::Map(values) => values
                    .iter()
                    .map(|(key, value)| Ok(((#key)?, (#item)?)))
                    .collect::<Result<std::collections::HashMap<_, _>, #lockgate::CallError>>(),
                _ => #mismatch,
            })
        }
        TypeDefKind::FixedLengthList(ty, size) => {
            let values = (0..*size)
                .map(|index| format_ident!("item_{index}"))
                .collect::<Vec<_>>();
            let items = values
                .iter()
                .map(|value| lift_value(types, *ty, quote!(#value), lockgate))
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(match #value {
                #lockgate::Value::FixedLengthList(values) => match values.as_slice() {
                    [#(#values),*] => Ok([#((#items)?),*]),
                    _ => #mismatch,
                },
                _ => #mismatch,
            })
        }
        TypeDefKind::Flags(flags) => {
            let ty = types.required_named_type(id, "flags")?;
            let cases = flags.flags.iter().map(|flag| {
                let name = &flag.name;
                let constant = format_ident!("{}", name.to_shouty_snake_case());
                quote!(#name => lifted |= #ty::#constant)
            });
            quote!(match #value {
                #lockgate::Value::Flags(names) => {
                    let mut lifted = #ty::empty();
                    for name in names {
                        match name.as_str() {
                            #(#cases,)*
                            _ => Err(#lockgate::CallError::shape(#message))?,
                        }
                    }
                    Ok(lifted)
                }
                _ => #mismatch,
            })
        }
        TypeDefKind::Type(ty) => lift_value(types, *ty, value, lockgate)?,
        TypeDefKind::Resource => return Err(unsupported_codegen_type("resource")),
        TypeDefKind::Handle(_) => return Err(unsupported_codegen_type("resource handle")),
        TypeDefKind::Future(_) => return Err(unsupported_codegen_type("future")),
        TypeDefKind::Stream(_) => return Err(unsupported_codegen_type("stream")),
        TypeDefKind::Unknown => return Err(unsupported_codegen_type("unknown")),
    })
}

fn lift_optional_value(
    types: &ExportTypeContext<'_>,
    ty: Option<Type>,
    value: TokenStream2,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    match ty {
        Some(ty) => {
            let lifted = lift_value(types, ty, value, lockgate)?;
            Ok(quote!((#lifted)?))
        }
        None => Ok(quote!(())),
    }
}

struct ExportTypeContext<'a> {
    resolve: &'a Resolve,
    interface: InterfaceId,
    type_names: BTreeMap<TypeId, Ident>,
    exported_modules: &'a BTreeMap<InterfaceId, Ident>,
}

impl ExportTypeContext<'_> {
    fn is_local(&self, id: TypeId) -> bool {
        matches!(
            self.resolve.types[id].owner,
            TypeOwner::Interface(owner) if owner == self.interface
        ) || self.resolve.types[id].owner == TypeOwner::None
    }

    fn named_type(&self, id: TypeId) -> syn::Result<Option<TokenStream2>> {
        let definition = &self.resolve.types[id];
        let Some(name) = definition.name.as_deref() else {
            return Ok(None);
        };
        let name = format_ident!("{}", rust_type_ident(name));

        match definition.owner {
            TypeOwner::Interface(owner) if owner != self.interface => {
                let Some(module) = self.exported_modules.get(&owner) else {
                    let type_name = definition.name.as_deref().unwrap_or("<unnamed>");
                    let interface_name = self
                        .resolve
                        .id_of(owner)
                        .unwrap_or_else(|| "<unnamed>".to_owned());
                    return Err(syn::Error::new(
                        proc_macro2::Span::call_site(),
                        format!(
                            "host role code generation cannot reference type `{type_name}` from interface `{interface_name}` because that interface is not exported by the selected world"
                        ),
                    ));
                };
                Ok(Some(quote!(super::#module::#name)))
            }
            TypeOwner::World(_) => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "host role code generation cannot reference world-owned type `{}` from an exported interface",
                    definition.name.as_deref().unwrap_or("<unnamed>")
                ),
            )),
            TypeOwner::Interface(_) | TypeOwner::None => {
                let name = self.type_names.get(&id).cloned().unwrap_or(name);
                Ok(Some(quote!(#name)))
            }
        }
    }

    fn required_named_type(&self, id: TypeId, kind: &str) -> syn::Result<TokenStream2> {
        self.named_type(id)?
            .ok_or_else(|| unnamed_codegen_type(kind))
    }
}

fn unnamed_codegen_type(kind: &str) -> syn::Error {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        format!("host role code generation requires {kind} types to be named"),
    )
}

fn unsupported_codegen_type(kind: &str) -> syn::Error {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        format!("host role code generation does not support {kind} values"),
    )
}

struct ImportedInterface {
    path: Vec<Ident>,
    public_module: Ident,
    public_types: Vec<Ident>,
}

struct ExportedInterface {
    id: InterfaceId,
    public_module: Ident,
    interface_name: String,
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

    let mut exported_interfaces = Vec::new();
    for (key, item) in &resolve.worlds[world].exports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let interface = &resolve.interfaces[*id];
        let Some(name) = interface.name.as_deref() else {
            continue;
        };
        let public_module = format_ident!("{}", rust_ident(name));
        if !module_names.insert(public_module.to_string()) {
            return Err(syn::Error::new_spanned(
                &input.world,
                format!("two WIT interfaces map to module `{public_module}`"),
            ));
        }
        exported_interfaces.push(ExportedInterface {
            id: *id,
            public_module,
            interface_name: resolve.name_world_key(key),
        });
    }

    if input.imports.is_none() && !interfaces.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.world,
            "lockgate::host_bindings! requires `imports` and `data` for a world that imports interfaces",
        ));
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
    let world_exports = std::mem::take(&mut resolve.worlds[world].exports);
    let generated = options.generate(&mut resolve, world);
    resolve.worlds[world].exports = world_exports;
    let generated =
        generated.map_err(|error| syn::Error::new_spanned(&input.world, error.to_string()))?;
    let mut generated = syn::parse_file(&generated).map_err(|error| {
        syn::Error::new_spanned(
            &input.world,
            format!("failed to read generated Wasmtime bindings: {error}"),
        )
    })?;

    if input.imports.is_some() {
        for interface in &interfaces {
            let items = nested_module_items_mut(&mut generated.items, &interface.path).ok_or_else(
                || {
                    syn::Error::new_spanned(
                        &input.world,
                        format!(
                            "could not locate generated host interface `{}`",
                            interface.public_module
                        ),
                    )
                },
            )?;
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
    }

    let generated_items = generated.items;
    let import_aliases = input.imports.as_ref().map(|config| {
        let imports = &config.imports;
        let data = &config.data;
        quote! {
            type __LockgateImports = #imports;
            type __LockgateData = #data;
        }
    });
    let public_modules = input
        .imports
        .as_ref()
        .map(|_| {
            interfaces
                .iter()
                .map(|interface| {
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
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let exported_modules = exported_interfaces
        .iter()
        .map(|interface| (interface.id, interface.public_module.clone()))
        .collect::<BTreeMap<_, _>>();
    let export_modules = exported_interfaces
        .iter()
        .map(|interface| export_module(&resolve, interface, &exported_modules, &lockgate))
        .collect::<syn::Result<Vec<_>>>()?;
    let host_imports_impl = input.imports.as_ref().map(|config| {
        let imports = &config.imports;
        let data = &config.data;
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
        quote! {
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
        }
    });

    Ok(quote! {
        #[doc(hidden)]
        pub mod __lockgate_host_bindings {
            use super::*;
            #import_aliases
            #(#generated_items)*
        }

        #(#public_modules)*
        #(#export_modules)*
        #host_imports_impl
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
            FnArg::Typed(store) => matches!(store.ty.as_ref(), SynType::Reference(_)),
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

fn future_output(output: &ReturnType) -> syn::Result<SynType> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "generated host method is missing its Future return type",
        ));
    };
    let SynType::ImplTrait(TypeImplTrait { bounds, .. }) = ty.as_ref() else {
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

#[cfg(test)]
mod tests {
    use super::HostBindingsInput;

    #[test]
    fn accepts_import_options_as_a_pair_or_not_at_all() {
        let imports: HostBindingsInput =
            syn::parse_str(r#"{ path: "wit", world: "plugin", imports: Imports, data: Data }"#)
                .unwrap();
        assert!(imports.imports.is_some());

        let exports: HostBindingsInput =
            syn::parse_str(r#"{ path: "wit", world: "plugin" }"#).unwrap();
        assert!(exports.imports.is_none());
    }

    #[test]
    fn rejects_half_of_the_import_option_pair() {
        for options in [
            r#"{ path: "wit", world: "plugin", imports: Imports }"#,
            r#"{ path: "wit", world: "plugin", data: Data }"#,
        ] {
            let error = match syn::parse_str::<HostBindingsInput>(options) {
                Ok(_) => panic!("half an import option pair was accepted"),
                Err(error) => error,
            };
            assert!(error.to_string().ends_with(
                "lockgate::host_bindings! options `imports` and `data` must be specified together"
            ));
        }
    }
}
