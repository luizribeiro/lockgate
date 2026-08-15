//! Private proc-macro implementations for the Lockgate guest facade.

use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use syn::{Ident, parse_macro_input};

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

    quote! {
        __lockgate_wit_export!(#plugin);
        // This mirrors wit-bindgen's keep-alive pattern for `cabi_realloc`.
        // Builds on the current toolchain are byte-identical without it, so it
        // is retained only as cheap insurance against linker or toolchain
        // behavior changes.
        #facade::__lockgate_runtime_keepalive!();

        const __LOCKGATE_PLUGIN_MANIFEST: #facade::__private::Manifest =
            #facade::__private::Manifest {
                id: <#plugin as #facade::Plugin>::ID,
                name: match <#plugin as #facade::Plugin>::NAME {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_NAME") {
                        ::core::option::Option::Some(value) if !value.is_empty() => value,
                        _ => panic!("Lockgate plugin Cargo name is missing or empty; set name in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => value,
                    #facade::MetadataSource::Absent => panic!("Lockgate plugin name cannot use MetadataSource::Absent because name is required by the wire format"),
                },
                version: match <#plugin as #facade::Plugin>::VERSION {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_VERSION") {
                        ::core::option::Option::Some(value) if !value.is_empty() => value,
                        _ => panic!("Lockgate plugin Cargo version is missing or empty; set version in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => value,
                    #facade::MetadataSource::Absent => panic!("Lockgate plugin version cannot use MetadataSource::Absent because version is required by the wire format"),
                },
                description: match <#plugin as #facade::Plugin>::DESCRIPTION {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_DESCRIPTION") {
                        ::core::option::Option::Some(value) if !value.is_empty() => ::core::option::Option::Some(value),
                        _ => panic!("Lockgate plugin Cargo description is missing or empty; set description in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => ::core::option::Option::Some(value),
                    #facade::MetadataSource::Absent => ::core::option::Option::None,
                },
                license: match <#plugin as #facade::Plugin>::LICENSE {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_LICENSE") {
                        ::core::option::Option::Some(value) if !value.is_empty() => ::core::option::Option::Some(value),
                        _ => panic!("Lockgate plugin Cargo license is missing or empty; set license in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => ::core::option::Option::Some(value),
                    #facade::MetadataSource::Absent => ::core::option::Option::None,
                },
                repository: match <#plugin as #facade::Plugin>::REPOSITORY {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_REPOSITORY") {
                        ::core::option::Option::Some(value) if !value.is_empty() => ::core::option::Option::Some(value),
                        _ => panic!("Lockgate plugin Cargo repository is missing or empty; set repository in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => ::core::option::Option::Some(value),
                    #facade::MetadataSource::Absent => ::core::option::Option::None,
                },
                homepage: match <#plugin as #facade::Plugin>::HOMEPAGE {
                    #facade::MetadataSource::Cargo => match option_env!("CARGO_PKG_HOMEPAGE") {
                        ::core::option::Option::Some(value) if !value.is_empty() => ::core::option::Option::Some(value),
                        _ => panic!("Lockgate plugin Cargo homepage is missing or empty; set homepage in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent"),
                    },
                    #facade::MetadataSource::Explicit(value) => ::core::option::Option::Some(value),
                    #facade::MetadataSource::Absent => ::core::option::Option::None,
                },
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
