//! Private proc-macro implementations for the Lockgate guest facade.

use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{Ident, LitStr, parse_macro_input};

#[proc_macro]
pub fn export(input: TokenStream) -> TokenStream {
    let plugin = parse_macro_input!(input as Ident);
    let metadata_section = PLUGIN_METADATA_SECTION;
    let needs_section = PLUGIN_NEEDS_SECTION;
    let facade = match crate_name("lockgate-plugin") {
        Ok(FoundCrate::Itself) => quote!(::lockgate_plugin),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            quote!(::#name)
        }
        Err(error) => {
            return syn::Error::new(plugin.span(), error.to_string())
                .into_compile_error()
                .into();
        }
    };
    let name = resolve_metadata_source(&facade, &plugin, "name", "CARGO_PKG_NAME", true);
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
        __lockgate_wit_export!(#plugin);

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
        #[used]
        #[unsafe(link_section = #metadata_section)]
        static __LOCKGATE_PLUGIN_METADATA: [u8; __LOCKGATE_PLUGIN_METADATA_LEN] =
            #facade::__private::metadata_bytes(&__LOCKGATE_PLUGIN_MANIFEST);

        const __LOCKGATE_PLUGIN_NEEDS_LEN: usize =
            #facade::__private::needs_len(&__LOCKGATE_PLUGIN_MANIFEST.needs);
        #[used]
        #[unsafe(link_section = #needs_section)]
        static __LOCKGATE_PLUGIN_NEEDS: [u8; __LOCKGATE_PLUGIN_NEEDS_LEN] =
            #facade::__private::needs_bytes(&__LOCKGATE_PLUGIN_MANIFEST.needs);
    }
    .into()
}

fn resolve_metadata_source(
    facade: &TokenStream2,
    plugin: &Ident,
    field: &str,
    env_var: &str,
    required: bool,
) -> TokenStream2 {
    let associated_const = format_ident!("{}", field.to_ascii_uppercase());
    let env_var = LitStr::new(env_var, Span::call_site());
    let cargo_error = LitStr::new(
        &format!(
            "Lockgate plugin Cargo {field} is missing or empty; set {field} in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"
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
                "Lockgate plugin {field} cannot use MetadataSource::Absent because {field} is required by the wire format"
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
