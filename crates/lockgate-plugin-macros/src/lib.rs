//! Private proc-macro implementations for the Lockgate guest facade.

use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Path, Token, braced, parse_macro_input};
use wit_bindgen_core::wit_parser::{
    Handle, InterfaceId, Resolve, Type, TypeDefKind, TypeId, UnresolvedPackageGroup, WorldId,
    WorldItem, WorldKey,
};
use wit_bindgen_core::{Files, WorldGenerator};
use wit_bindgen_rust::{Opts, WithOption};

#[proc_macro]
pub fn generate(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as GenerateInput);
    generate_bindings(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct GenerateInput {
    source: GenerateSource,
    world: LitStr,
    facade: Option<Path>,
    with: Vec<(LitStr, Path)>,
}

enum GenerateSource {
    Path(LitStr),
    Inline(LitStr),
}

impl Parse for GenerateInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let content;
        braced!(content in input);
        let mut path = None;
        let mut inline = None;
        let mut world = None;
        let mut facade = None;
        let mut with = None;
        while !content.is_empty() {
            let field: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            match field.to_string().as_str() {
                "path" => {
                    let value = content.parse::<LitStr>()?;
                    if path.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate generate! field"));
                    }
                }
                "inline" => {
                    let value = content.parse::<LitStr>()?;
                    if inline.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate generate! field"));
                    }
                }
                "world" => {
                    let value = content.parse::<LitStr>()?;
                    if world.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate generate! field"));
                    }
                }
                "facade" => {
                    let value = content.parse::<Path>()?;
                    if facade.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate generate! field"));
                    }
                }
                "with" => {
                    let value = parse_with(&content)?;
                    if with.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate generate! field"));
                    }
                }
                _ => return Err(syn::Error::new(field.span(), "unknown generate! field")),
            }
            if content.is_empty() {
                break;
            }
            content.parse::<Token![,]>()?;
        }
        let source = match (path, inline) {
            (Some(path), None) => GenerateSource::Path(path),
            (None, Some(inline)) => GenerateSource::Inline(inline),
            (Some(_), Some(inline)) => {
                return Err(syn::Error::new(
                    inline.span(),
                    "generate! requires exactly one of `path` or `inline`",
                ));
            }
            (None, None) => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "generate! requires exactly one of `path` or `inline`",
                ));
            }
        };
        Ok(Self {
            source,
            world: world.ok_or_else(|| input.error("generate! requires `world`"))?,
            facade,
            with: with.unwrap_or_default(),
        })
    }
}

fn parse_with(input: ParseStream<'_>) -> syn::Result<Vec<(LitStr, Path)>> {
    let content;
    braced!(content in input);
    let mut mappings = Vec::new();
    while !content.is_empty() {
        let interface = content.parse::<LitStr>()?;
        content.parse::<Token![:]>()?;
        let path = content.parse::<Path>()?;
        mappings.push((interface, path));
        if content.is_empty() {
            break;
        }
        content.parse::<Token![,]>()?;
    }
    Ok(mappings)
}

fn generate_bindings(input: GenerateInput) -> syn::Result<TokenStream2> {
    let mut resolve = Resolve::default();
    let (plugin_package, sources) = match &input.source {
        GenerateSource::Path(path) => {
            let manifest_dir = PathBuf::from(
                std::env::var("CARGO_MANIFEST_DIR")
                    .map_err(|error| syn::Error::new(path.span(), error))?,
            );
            let source_path = manifest_dir.join(path.value());
            let (package, sources) = resolve
                .push_path(&source_path)
                .map_err(|error| syn::Error::new(path.span(), format!("{error:#}")))?;
            (package, Some(sources))
        }
        GenerateSource::Inline(source) => {
            let source_text = source.value();
            let package = resolve
                .push_group(
                    UnresolvedPackageGroup::parse("lockgate-inline.wit", &source_text)
                        .map_err(|error| syn::Error::new(source.span(), format!("{error:#}")))?,
                )
                .map_err(|error| syn::Error::new(source.span(), format!("{error:#}")))?;
            (package, None)
        }
    };
    let plugin_world = resolve
        .select_world(&[plugin_package], Some(&input.world.value()))
        .map_err(|error| syn::Error::new(input.world.span(), format!("{error:#}")))?;
    validate_guest_world(&resolve, plugin_world, input.world.span())?;
    let plugin_world_name = resolve.worlds[plugin_world].name.clone();
    let plugin_package = &resolve.packages[plugin_package].name;
    let mut plugin_world_reference = format!(
        "{}:{}/{}",
        plugin_package.namespace, plugin_package.name, plugin_world_name
    );
    if let Some(version) = &plugin_package.version {
        plugin_world_reference.push_str(&format!("@{version}"));
    }

    resolve
        .push_group(
            UnresolvedPackageGroup::parse("lockgate-config.wit", lockgate_schema::CONFIG_WIT)
                .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?,
        )
        .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?;
    let wrapper = format!(
        "package lockgate:generated;\nworld plugin {{\n  include {plugin_world_reference};\n  import lockgate:config/settings;\n  export lockgate:config/schema;\n}}"
    );
    let wrapper_package = resolve
        .push_group(
            UnresolvedPackageGroup::parse("lockgate-generated.wit", &wrapper)
                .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?,
        )
        .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?;
    let world = resolve
        .select_world(&[wrapper_package], Some("plugin"))
        .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?;

    let facade = resolve_facade(input.facade.as_ref(), Span::call_site())?;
    let with = input
        .with
        .iter()
        .map(|(interface, path)| {
            (
                interface.value(),
                WithOption::Path(quote!(#path).to_string()),
            )
        })
        .collect();
    let options = Opts {
        export_macro_name: Some("__lockgate_wit_export".into()),
        runtime_path: Some(quote!(#facade::__wit_bindgen::rt).to_string()),
        generate_all: true,
        with,
        ..Opts::default()
    };
    let mut files = Files::default();
    options
        .build()
        .generate(&mut resolve, world, &mut files)
        .map_err(|error| syn::Error::new(Span::call_site(), format!("{error:#}")))?;
    let (_, generated) = files
        .iter()
        .next()
        .ok_or_else(|| syn::Error::new(Span::call_site(), "guest binding generation was empty"))?;
    let mut output = std::str::from_utf8(generated)
        .map_err(|error| syn::Error::new(Span::call_site(), error))?
        .parse::<TokenStream2>()
        .map_err(|error| syn::Error::new(Span::call_site(), error))?;
    if let Some(sources) = sources {
        for file in sources.paths() {
            let file = LitStr::new(&file.to_string_lossy(), Span::call_site());
            output.extend(quote!(
                const _: &[u8] = include_bytes!(#file);
            ));
        }
    }
    output.extend(quote! {
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub extern "Rust" fn __lockgate_settings_json() -> #facade::alloc::string::String {
            match lockgate::config::settings::get_json() {
                ::core::result::Result::Ok(json) => json,
                ::core::result::Result::Err(error) => {
                    panic!("Lockgate validated settings are unavailable: {error:?}")
                }
            }
        }
    });
    Ok(output)
}

fn resolve_facade(override_path: Option<&Path>, error_span: Span) -> syn::Result<TokenStream2> {
    if let Some(path) = override_path {
        return Ok(quote!(#path));
    }
    match crate_name("lockgate-plugin") {
        Ok(FoundCrate::Itself) => Ok(quote!(::lockgate_plugin)),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            Ok(quote!(::#name))
        }
        Err(error) => Err(syn::Error::new(error_span, error.to_string())),
    }
}

fn validate_guest_world(resolve: &Resolve, world: WorldId, span: Span) -> syn::Result<()> {
    let world = &resolve.worlds[world];
    for (key, item) in &world.exports {
        let export_name = world_key_name(resolve, key);
        match item {
            WorldItem::Interface { id, .. } => {
                validate_guest_interface(resolve, *id, &export_name, span)?;
            }
            WorldItem::Function(_) => {
                return unsupported_export(
                    span,
                    &world.name,
                    &export_name,
                    "world-level exported function",
                );
            }
            WorldItem::Type { .. } => {
                return unsupported_export(
                    span,
                    &world.name,
                    &format!("<type {export_name}>"),
                    "world-level exported type",
                );
            }
        }
    }
    Ok(())
}

fn validate_guest_interface(
    resolve: &Resolve,
    interface: InterfaceId,
    interface_name: &str,
    span: Span,
) -> syn::Result<()> {
    for function in resolve.interfaces[interface].functions.values() {
        let types = function
            .params
            .iter()
            .map(|param| param.ty)
            .chain(function.result);
        for ty in types {
            if let Some(offending_type) = forbidden_type(resolve, ty) {
                return unsupported_export(span, interface_name, &function.name, &offending_type);
            }
        }
    }
    for (name, id) in &resolve.interfaces[interface].types {
        let resolved = resolve_alias(resolve, *id);
        if matches!(resolve.types[resolved].kind, TypeDefKind::Resource) {
            return unsupported_export(
                span,
                interface_name,
                &format!("<type {name}>"),
                &format!("resource {}", type_name(resolve, resolved)),
            );
        }
    }
    Ok(())
}

fn unsupported_export<T>(
    span: Span,
    interface: &str,
    function: &str,
    offending_type: &str,
) -> syn::Result<T> {
    let mut message = format!(
        "[admission.unsupported-export] unsupported export `{interface}#{function}`: offending type `{offending_type}` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
    );
    if matches!(offending_type, "future" | "stream") {
        message.push_str(
            ". Async WIT functions are supported; this restriction applies to `future`, `stream`, and `error-context` value types and resource handles crossing the invocation boundary, not to the function's async declaration.",
        );
    }
    Err(syn::Error::new(span, message))
}

fn forbidden_type(resolve: &Resolve, ty: Type) -> Option<String> {
    let id = match ty {
        Type::ErrorContext => return Some("error-context".into()),
        Type::Id(id) => resolve_alias(resolve, id),
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::S64
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::String => return None,
    };
    match &resolve.types[id].kind {
        TypeDefKind::Resource => Some(format!("resource {}", type_name(resolve, id))),
        TypeDefKind::Handle(handle) => Some(match handle {
            Handle::Own(resource) => format!("own<{}>", type_name(resolve, *resource)),
            Handle::Borrow(resource) => format!("borrow<{}>", type_name(resolve, *resource)),
        }),
        TypeDefKind::Record(record) => record
            .fields
            .iter()
            .find_map(|field| forbidden_type(resolve, field.ty)),
        TypeDefKind::Tuple(tuple) => tuple
            .types
            .iter()
            .find_map(|ty| forbidden_type(resolve, *ty)),
        TypeDefKind::Variant(variant) => variant
            .cases
            .iter()
            .filter_map(|case| case.ty)
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Option(ty)
        | TypeDefKind::List(ty)
        | TypeDefKind::FixedLengthList(ty, _)
        | TypeDefKind::Type(ty) => forbidden_type(resolve, *ty),
        TypeDefKind::Result(result) => result
            .ok
            .into_iter()
            .chain(result.err)
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Map(key, value) => [*key, *value]
            .into_iter()
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Future(_) => Some("future".into()),
        TypeDefKind::Stream(_) => Some("stream".into()),
        TypeDefKind::Flags(_) | TypeDefKind::Enum(_) | TypeDefKind::Unknown => None,
    }
}

fn resolve_alias(resolve: &Resolve, id: TypeId) -> TypeId {
    match resolve.types[id].kind {
        TypeDefKind::Type(Type::Id(target)) => resolve_alias(resolve, target),
        _ => id,
    }
}

fn type_name(resolve: &Resolve, id: TypeId) -> String {
    resolve.types[id]
        .name
        .clone()
        .unwrap_or_else(|| format!("type-{}", id.index()))
}

fn world_key_name(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(name) => name.clone(),
        WorldKey::Interface(id) => resolve
            .id_of(*id)
            .unwrap_or_else(|| format!("interface-{}", id.index())),
    }
}

#[proc_macro]
pub fn export(input: TokenStream) -> TokenStream {
    let ExportInput { plugin, facade } = parse_macro_input!(input as ExportInput);
    let metadata_section = PLUGIN_METADATA_SECTION;
    let needs_section = PLUGIN_NEEDS_SECTION;
    let facade = match resolve_facade(facade.as_ref(), plugin.span()) {
        Ok(facade) => facade,
        Err(error) => {
            return error.into_compile_error().into();
        }
    };
    let name = resolve_metadata_source(&facade, &plugin, "display_name", "CARGO_PKG_NAME", true);
    let version = resolve_metadata_source(&facade, &plugin, "version", "CARGO_PKG_VERSION", true);
    let description = resolve_metadata_source(
        &facade,
        &plugin,
        "description",
        "CARGO_PKG_DESCRIPTION",
        false,
    );
    let license = resolve_metadata_source(&facade, &plugin, "license", "CARGO_PKG_LICENSE", false);
    let repository = resolve_metadata_source(
        &facade,
        &plugin,
        "repository",
        "CARGO_PKG_REPOSITORY",
        false,
    );
    let homepage =
        resolve_metadata_source(&facade, &plugin, "homepage", "CARGO_PKG_HOMEPAGE", false);

    quote! {
        impl exports::lockgate::config::schema::Guest for #plugin {
            fn settings_schema() -> #facade::alloc::string::String {
                #facade::__private::settings_schema::<#plugin>()
            }
        }

        #[cfg(target_arch = "wasm32")]
        __lockgate_wit_export!(#plugin);

        const __LOCKGATE_SETTINGS_POLICY_CHECK: () =
            #facade::__private::validate_settings_policy::<#plugin>();

        const __LOCKGATE_PLUGIN_MANIFEST: #facade::__private::Manifest =
            #facade::__private::Manifest {
                id: <#plugin as #facade::Plugin>::ID,
                name: #name,
                version: #version,
                description: #description,
                license: #license,
                repository: #repository,
                homepage: #homepage,
                needs: <#plugin as #facade::Plugin>::NEEDS,
            };

        const __LOCKGATE_PLUGIN_METADATA_LEN: usize =
            #facade::__private::metadata_len(&__LOCKGATE_PLUGIN_MANIFEST);
        #[cfg(target_arch = "wasm32")]
        #[used]
        #[unsafe(link_section = #metadata_section)]
        static __LOCKGATE_PLUGIN_METADATA: [u8; __LOCKGATE_PLUGIN_METADATA_LEN] =
            #facade::__private::metadata_bytes(&__LOCKGATE_PLUGIN_MANIFEST);

        const __LOCKGATE_PLUGIN_NEEDS_LEN: usize =
            #facade::__private::needs_len(&__LOCKGATE_PLUGIN_MANIFEST.needs);
        #[cfg(target_arch = "wasm32")]
        #[used]
        #[unsafe(link_section = #needs_section)]
        static __LOCKGATE_PLUGIN_NEEDS: [u8; __LOCKGATE_PLUGIN_NEEDS_LEN] =
            #facade::__private::needs_bytes(&__LOCKGATE_PLUGIN_MANIFEST.needs);
    }
    .into()
}

struct ExportInput {
    plugin: Ident,
    facade: Option<Path>,
}

impl Parse for ExportInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let plugin = input.parse()?;
        let mut facade = None;
        while !input.is_empty() {
            input.parse::<Token![;]>()?;
            let field = input.parse::<Ident>()?;
            input.parse::<Token![=]>()?;
            match field.to_string().as_str() {
                "facade" => {
                    let value = input.parse::<Path>()?;
                    if facade.replace(value).is_some() {
                        return Err(syn::Error::new(field.span(), "duplicate export! option"));
                    }
                }
                _ => return Err(syn::Error::new(field.span(), "unknown export! option")),
            }
        }
        Ok(Self { plugin, facade })
    }
}

fn resolve_metadata_source(
    facade: &TokenStream2,
    plugin: &Ident,
    field: &str,
    env_var: &str,
    required: bool,
) -> TokenStream2 {
    let associated_const = format_ident!("{}", field.to_ascii_uppercase());
    let (authoring_field, wire_field, source_help) = if field == "display_name" {
        (
            "display name",
            "name",
            "Plugin::DISPLAY_NAME = MetadataSource::Explicit(...) or Plugin::DISPLAY_NAME = MetadataSource::Absent",
        )
    } else {
        (
            field,
            field,
            "MetadataSource::Explicit(...) or MetadataSource::Absent",
        )
    };
    let env_var = LitStr::new(env_var, Span::call_site());
    let cargo_error = LitStr::new(
        &format!(
            "Lockgate plugin Cargo {wire_field} is missing or empty; set {wire_field} in Cargo.toml, or declare {source_help}"
        ),
        Span::call_site(),
    );
    let present = if required {
        quote!(value)
    } else {
        quote!(::core::option::Option::Some(value))
    };
    let absent = if required {
        let absent_error = LitStr::new(
            &format!(
                "Lockgate plugin {authoring_field} cannot use MetadataSource::Absent because {wire_field} is required by the wire format"
            ),
            Span::call_site(),
        );
        quote!(panic!(#absent_error))
    } else {
        quote!(::core::option::Option::None)
    };

    quote! {
        match <#plugin as #facade::Plugin>::#associated_const {
            #facade::MetadataSource::Cargo => match option_env!(#env_var) {
                ::core::option::Option::Some(value) if !value.is_empty() => #present,
                _ => panic!(#cargo_error),
            },
            #facade::MetadataSource::Explicit(value) => #present,
            #facade::MetadataSource::Absent => #absent,
        }
    }
}

#[cfg(test)]
mod macro_input_tests {
    use super::{ExportInput, GenerateInput, GenerateSource};

    #[test]
    fn accepts_a_path_source() {
        let input = syn::parse_str::<GenerateInput>(r#"{ path: "wit", world: "plugin" }"#)
            .expect("path source should parse");

        assert!(matches!(
            input.source,
            GenerateSource::Path(path) if path.value() == "wit"
        ));
        assert_eq!(input.world.value(), "plugin");
    }

    #[test]
    fn accepts_an_inline_source() {
        let input = syn::parse_str::<GenerateInput>(
            r#"{ inline: "package test:inline; world plugin {}", world: "plugin" }"#,
        )
        .expect("inline source should parse");

        assert!(matches!(
            input.source,
            GenerateSource::Inline(source) if source.value().starts_with("package test:inline")
        ));
        assert_eq!(input.world.value(), "plugin");
    }

    #[test]
    fn generate_accepts_an_explicit_facade_path() {
        let input = syn::parse_str::<GenerateInput>(
            r#"{
                inline: "package test:inline; world plugin {}",
                world: "plugin",
                facade: ::sage_plugin,
            }"#,
        )
        .expect("facade override should parse");
        let facade = input.facade.expect("facade override should be retained");

        assert!(facade.leading_colon.is_some());
        assert_eq!(facade.segments.last().unwrap().ident, "sage_plugin");
    }

    #[test]
    fn generate_accepts_with_mappings_and_a_trailing_comma() {
        let input = syn::parse_str::<GenerateInput>(
            r#"{
                inline: "package test:inline; world plugin {}",
                world: "plugin",
                with: {
                    "sage:agent/types@0.2.0": ::sage_plugin::types,
                    "some:pkg/iface": ::other::module,
                },
            }"#,
        )
        .expect("with mappings should parse");

        assert_eq!(input.with.len(), 2);
        assert_eq!(input.with[0].0.value(), "sage:agent/types@0.2.0");
        assert!(input.with[0].1.leading_colon.is_some());
        assert_eq!(input.with[0].1.segments.last().unwrap().ident, "types");
        assert_eq!(input.with[1].0.value(), "some:pkg/iface");
        assert_eq!(input.with[1].1.segments.last().unwrap().ident, "module");
    }

    #[test]
    fn generate_accepts_an_empty_with_map() {
        let input = syn::parse_str::<GenerateInput>(
            r#"{
                path: "wit",
                world: "plugin",
                with: {},
            }"#,
        )
        .expect("an empty with map should parse");

        assert!(input.with.is_empty());
    }

    #[test]
    fn generate_keeps_with_empty_when_omitted() {
        let input = syn::parse_str::<GenerateInput>(r#"{ path: "wit", world: "plugin" }"#)
            .expect("with should remain optional");

        assert!(input.with.is_empty());
    }

    #[test]
    fn rejects_duplicate_with_fields() {
        let error = syn::parse_str::<GenerateInput>(
            r#"{
                path: "wit",
                world: "plugin",
                with: {},
                with: { "some:pkg/iface": ::other::module },
            }"#,
        )
        .err()
        .expect("duplicate with fields should fail");

        assert_eq!(error.to_string(), "duplicate generate! field");
    }

    #[test]
    fn export_accepts_an_explicit_facade_path() {
        let input = syn::parse_str::<ExportInput>("Fixture; facade = ::sage_plugin")
            .expect("facade override should parse");
        let facade = input.facade.expect("facade override should be retained");

        assert_eq!(input.plugin, "Fixture");
        assert!(facade.leading_colon.is_some());
        assert_eq!(facade.segments.last().unwrap().ident, "sage_plugin");
    }

    #[test]
    fn export_keeps_facade_auto_detection_as_the_default() {
        let input = syn::parse_str::<ExportInput>("Fixture").expect("plugin should parse");

        assert_eq!(input.plugin, "Fixture");
        assert!(input.facade.is_none());
    }

    #[test]
    fn rejects_path_and_inline_together() {
        let error = syn::parse_str::<GenerateInput>(
            r#"{ path: "wit", inline: "package test:inline;", world: "plugin" }"#,
        )
        .err()
        .expect("path and inline should conflict");

        assert_eq!(
            error.to_string(),
            "generate! requires exactly one of `path` or `inline`"
        );
    }

    #[test]
    fn rejects_a_missing_source() {
        let error = syn::parse_str::<GenerateInput>(r#"{ world: "plugin" }"#)
            .err()
            .expect("a source should be required");

        assert_eq!(
            error.to_string(),
            "generate! requires exactly one of `path` or `inline`"
        );
    }
}
