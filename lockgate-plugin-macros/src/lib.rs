use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::collections::BTreeMap;
use syn::{Ident, LitByteStr, LitStr, Token, braced, parse::Parse, parse::ParseStream};

/// Generates guest bindings and embeds required Lockgate plugin metadata.
#[proc_macro]
pub fn bindings(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as BindingsInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

struct BindingsInput {
    path: LitStr,
    world: LitStr,
    metadata: MetadataInput,
}

struct MetadataInput {
    fields: BTreeMap<String, LitStr>,
}

impl Parse for BindingsInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let content;
        let options = if input.peek(syn::token::Brace) {
            braced!(content in input);
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after bindings options"));
            }
            &content
        } else {
            input
        };
        let mut path = None;
        let mut world = None;
        let mut metadata = None;
        while !options.is_empty() {
            let option: Ident = options.parse()?;
            options.parse::<Token![:]>()?;
            match option.to_string().as_str() {
                "path" => parse_once(&mut path, options.parse()?, &option)?,
                "world" => parse_once(&mut world, options.parse()?, &option)?,
                "metadata" => {
                    let metadata_content;
                    braced!(metadata_content in options);
                    parse_once(&mut metadata, metadata_content.parse()?, &option)?;
                }
                name => {
                    return Err(syn::Error::new(
                        option.span(),
                        format!("unsupported lockgate_plugin::bindings! option `{name}`"),
                    ));
                }
            }
            if options.is_empty() {
                break;
            }
            options.parse::<Token![,]>()?;
        }
        Ok(Self {
            path: path.ok_or_else(|| required(input, "path"))?,
            world: world.ok_or_else(|| required(input, "world"))?,
            metadata: metadata.ok_or_else(|| required(input, "metadata"))?,
        })
    }
}

impl Parse for MetadataInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut fields = BTreeMap::new();
        while !input.is_empty() {
            let field: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let value: LitStr = input.parse()?;
            let name = field.to_string();
            if !matches!(
                name.as_str(),
                "id" | "name" | "version" | "description" | "license" | "repository" | "homepage"
            ) {
                return Err(syn::Error::new(
                    field.span(),
                    format!("unsupported plugin metadata field `{name}`"),
                ));
            }
            if fields.insert(name.clone(), value).is_some() {
                return Err(syn::Error::new(
                    field.span(),
                    format!("duplicate plugin metadata field `{name}`"),
                ));
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        for required in ["id", "name", "version"] {
            if !fields.contains_key(required) {
                return Err(syn::Error::new(
                    input.span(),
                    format!("plugin metadata requires `{required}`"),
                ));
            }
        }
        Ok(Self { fields })
    }
}

fn expand(input: BindingsInput) -> syn::Result<TokenStream2> {
    let (plugin, runtime_path) = plugin_path(&input.path)?;
    let metadata = build_metadata(&input.metadata)?;
    let encoded = lockgate_schema::encode_plugin_metadata(&metadata)
        .map_err(|error| syn::Error::new_spanned(&input.world, error.to_string()))?;
    let bytes = LitByteStr::new(&encoded, Span::call_site());
    let length = encoded.len();
    let path = input.path;
    let world = input.world;
    let section = lockgate_schema::PLUGIN_METADATA_SECTION;
    Ok(quote! {
        #plugin::__wit_bindgen::generate!({
            path: #path,
            world: #world,
            generate_all,
            disable_custom_section_link_helpers: true,
            runtime_path: #runtime_path,
        });

        const _: () = {
            #[cfg(target_family = "wasm")]
            #[used]
            #[unsafe(link_section = #section)]
            static __LOCKGATE_PLUGIN_METADATA: [u8; #length] = *#bytes;
        };
    })
}

fn build_metadata(input: &MetadataInput) -> syn::Result<lockgate_schema::PluginMetadata> {
    let value = |name: &str| {
        input
            .fields
            .get(name)
            .expect("required metadata fields were checked")
            .value()
    };
    let mut metadata =
        lockgate_schema::PluginMetadata::new(value("id"), value("name"), value("version"))
            .map_err(|error| syn::Error::new_spanned(&input.fields["id"], error.to_string()))?;
    if let Some(value) = input.fields.get("description") {
        metadata = metadata.with_description(value.value());
    }
    if let Some(value) = input.fields.get("license") {
        metadata = metadata.with_license(value.value());
    }
    if let Some(value) = input.fields.get("repository") {
        metadata = metadata.with_repository(value.value());
    }
    if let Some(value) = input.fields.get("homepage") {
        metadata = metadata.with_homepage(value.value());
    }
    Ok(metadata)
}

fn plugin_path(span: &impl quote::ToTokens) -> syn::Result<(TokenStream2, String)> {
    match crate_name("lockgate-plugin") {
        Ok(FoundCrate::Itself) => Ok((quote!(crate), "crate::__wit_bindgen::rt".into())),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{}", name.replace('-', "_"));
            Ok((quote!(::#ident), format!("::{ident}::__wit_bindgen::rt")))
        }
        Err(error) => Err(syn::Error::new_spanned(
            span,
            format!("failed to locate lockgate-plugin: {error}"),
        )),
    }
}

fn parse_once<T>(slot: &mut Option<T>, value: T, option: &Ident) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            option.span(),
            format!("duplicate `{option}` option"),
        ));
    }
    Ok(())
}

fn required(input: ParseStream<'_>, option: &str) -> syn::Error {
    syn::Error::new(
        input.span(),
        format!("lockgate_plugin::bindings! requires `{option}`"),
    )
}
