//! Private proc-macro implementations for the Lockgate guest facade.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Ident, parse_macro_input};

#[proc_macro]
pub fn export(input: TokenStream) -> TokenStream {
    let plugin = parse_macro_input!(input as Ident);

    quote! {
        __lockgate_wit_export!(#plugin);

        const __LOCKGATE_PLUGIN_MANIFEST: ::lockgate_plugin::__private::Manifest =
            ::lockgate_plugin::__private::Manifest {
                id: <#plugin as ::lockgate_plugin::Plugin>::ID,
                name: match <#plugin as ::lockgate_plugin::Plugin>::NAME {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => env!("CARGO_PKG_NAME"),
                },
                version: match <#plugin as ::lockgate_plugin::Plugin>::VERSION {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => env!("CARGO_PKG_VERSION"),
                },
                description: match <#plugin as ::lockgate_plugin::Plugin>::DESCRIPTION {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => ::lockgate_plugin::__private::cargo_optional(
                        option_env!("CARGO_PKG_DESCRIPTION"),
                    ),
                },
                license: match <#plugin as ::lockgate_plugin::Plugin>::LICENSE {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => ::lockgate_plugin::__private::cargo_optional(
                        option_env!("CARGO_PKG_LICENSE"),
                    ),
                },
                repository: match <#plugin as ::lockgate_plugin::Plugin>::REPOSITORY {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => ::lockgate_plugin::__private::cargo_optional(
                        option_env!("CARGO_PKG_REPOSITORY"),
                    ),
                },
                homepage: match <#plugin as ::lockgate_plugin::Plugin>::HOMEPAGE {
                    ::core::option::Option::Some(value) => ::core::option::Option::Some(value),
                    ::core::option::Option::None => ::lockgate_plugin::__private::cargo_optional(
                        option_env!("CARGO_PKG_HOMEPAGE"),
                    ),
                },
                needs: <#plugin as ::lockgate_plugin::Plugin>::NEEDS,
            };

        const __LOCKGATE_PLUGIN_METADATA_LEN: usize =
            ::lockgate_plugin::__private::metadata_len(&__LOCKGATE_PLUGIN_MANIFEST);
        #[used]
        #[unsafe(link_section = "lockgate:plugin")]
        static __LOCKGATE_PLUGIN_METADATA: [u8; __LOCKGATE_PLUGIN_METADATA_LEN] =
            ::lockgate_plugin::__private::metadata_bytes(&__LOCKGATE_PLUGIN_MANIFEST);

        const __LOCKGATE_PLUGIN_NEEDS_LEN: usize =
            ::lockgate_plugin::__private::needs_len(&__LOCKGATE_PLUGIN_MANIFEST.needs);
        #[used]
        #[unsafe(link_section = "lockgate:needs")]
        static __LOCKGATE_PLUGIN_NEEDS: [u8; __LOCKGATE_PLUGIN_NEEDS_LEN] =
            ::lockgate_plugin::__private::needs_bytes(&__LOCKGATE_PLUGIN_MANIFEST.needs);
    }
    .into()
}
