use heck::ToUpperCamelCase;
use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::LitStr;
use wit_parser::Resolve;

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
    let (world_name, exports) = match load_world(&input, &world) {
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

    quote! {
        ::wasmtime::component::bindgen!(#input);

        const _: () = {
            impl #lockgate::__private::ComponentBinding for #binding {
                const WORLD: &'static str = #world;
                const EXPORTS: &'static [#lockgate::__private::BindingExport] =
                    &[#(#export_metadata),*];

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

fn load_world(
    input: &TokenStream2,
    world: &LitStr,
) -> syn::Result<(String, Vec<lockgate_schema::Export>)> {
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
    Ok((name, exports))
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
