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
        #facade::__lockgate_runtime_keepalive!();

        const __LOCKGATE_PLUGIN_MANIFEST: #facade::__private::Manifest =
            #facade::__private::Manifest {
                id: <#plugin as #facade::Plugin>::ID,
                name: match <#plugin as #facade::Plugin>::NAME {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => env!("CARGO_PKG_NAME"),
                },
                version: match <#plugin as #facade::Plugin>::VERSION {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => env!("CARGO_PKG_VERSION"),
                },
                description: match <#plugin as #facade::Plugin>::DESCRIPTION {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => #facade::__private::cargo_optional(
                        option_env!("CARGO_PKG_DESCRIPTION"),
                    ),
                },
                license: match <#plugin as #facade::Plugin>::LICENSE {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => #facade::__private::cargo_optional(
                        option_env!("CARGO_PKG_LICENSE"),
                    ),
                },
                repository: match <#plugin as #facade::Plugin>::REPOSITORY {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => #facade::__private::cargo_optional(
                        option_env!("CARGO_PKG_REPOSITORY"),
                    ),
                },
                homepage: match <#plugin as #facade::Plugin>::HOMEPAGE {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => #facade::__private::cargo_optional(
                        option_env!("CARGO_PKG_HOMEPAGE"),
                    ),
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
