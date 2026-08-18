use std::collections::BTreeMap;

use heck::ToKebabCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Variant};

pub(super) fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    reject_container_attributes(&input.attrs)?;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`ScopeRepr` can only be derived for enums",
        ));
    };
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`ScopeRepr` requires at least one variant; an empty closed scope domain is meaningless",
        ));
    }

    let mut names = BTreeMap::<String, proc_macro2::Span>::new();
    let mut variants = Vec::with_capacity(data.variants.len());
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.fields,
                format!(
                    "`ScopeRepr` requires unit variants; variant `{}` has fields",
                    variant.ident
                ),
            ));
        }

        let (wire_name, span) = wire_name(variant)?;
        validate_wire_name(&wire_name, span)?;
        if names.insert(wire_name.clone(), span).is_some() {
            return Err(syn::Error::new(
                span,
                format!("duplicate scope wire name `{wire_name}` after renaming"),
            ));
        }
        variants.push((&variant.ident, wire_name));
    }

    let lockgate_policy = super::lockgate_policy_path(&input.ident)?;
    expand_with_path(&input, &variants, &lockgate_policy)
}

fn expand_with_path(
    input: &DeriveInput,
    variants: &[(&syn::Ident, String)],
    lockgate_policy: &TokenStream,
) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let variant_idents = variants.iter().map(|(variant, _)| variant);
    let canonical_arms = variants
        .iter()
        .map(|(variant, wire_name)| quote!(Self::#variant => #wire_name));
    let parse_arms = variants.iter().map(
        |(variant, wire_name)| quote!(#wire_name => ::core::result::Result::Ok(Self::#variant)),
    );

    Ok(quote! {
        impl #impl_generics ::core::str::FromStr for #name #type_generics #where_clause {
            type Err = #lockgate_policy::ScopeError;

            fn from_str(value: &str) -> ::core::result::Result<Self, Self::Err> {
                match value {
                    #(#parse_arms,)*
                    value => ::core::result::Result::Err(
                        #lockgate_policy::ScopeError::unknown(value)
                    ),
                }
            }
        }

        impl #impl_generics #lockgate_policy::ScopeRepr for #name #type_generics #where_clause {
            fn canonical(&self) -> #lockgate_policy::__private::String {
                #lockgate_policy::__private::String::from(match self {
                    #(#canonical_arms,)*
                })
            }

            fn exhaustive_domain() -> ::core::option::Option<
                #lockgate_policy::ExhaustiveScopeDomain<Self>
            > {
                ::core::option::Option::Some(
                    #lockgate_policy::ExhaustiveScopeDomain::__from_derive(
                        #lockgate_policy::__private::vec![#(Self::#variant_idents),*]
                    )
                )
            }
        }
    })
}

fn reject_container_attributes(attributes: &[Attribute]) -> syn::Result<()> {
    if let Some(attribute) = attributes
        .iter()
        .find(|attribute| attribute.path().is_ident("scope"))
    {
        return Err(syn::Error::new_spanned(
            attribute,
            "`scope` attributes are only supported on enum variants",
        ));
    }
    Ok(())
}

fn wire_name(variant: &Variant) -> syn::Result<(String, proc_macro2::Span)> {
    let mut scope_attribute = None;
    let mut rename = None;

    for attribute in &variant.attrs {
        if !attribute.path().is_ident("scope") {
            continue;
        }
        if scope_attribute.replace(attribute).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                format!("duplicate `scope` attribute on variant `{}`", variant.ident),
            ));
        }

        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("rename") {
                let attribute = meta
                    .path
                    .get_ident()
                    .map_or_else(|| "unknown".to_owned(), ToString::to_string);
                return Err(meta.error(format!(
                    "unknown `scope` attribute `{attribute}`; expected `rename`"
                )));
            }
            if rename.is_some() {
                return Err(meta.error("duplicate `rename` scope attribute"));
            }
            rename = Some(meta.value()?.parse::<LitStr>()?);
            Ok(())
        })?;
        if rename.is_none() {
            return Err(syn::Error::new_spanned(
                attribute,
                "`scope` attribute requires `rename = \"...\"`",
            ));
        }
    }

    Ok(match rename {
        Some(rename) => (rename.value(), rename.span()),
        None => (
            variant.ident.to_string().to_kebab_case(),
            variant.ident.span(),
        ),
    })
}

fn validate_wire_name(name: &str, span: proc_macro2::Span) -> syn::Result<()> {
    if name.is_empty() {
        return Err(syn::Error::new(span, "scope wire name cannot be empty"));
    }

    let bytes = name.as_bytes();
    let valid = bytes[0].is_ascii_lowercase()
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !bytes.windows(2).any(|pair| pair == b"--");
    if !valid {
        return Err(syn::Error::new(
            span,
            format!(
                "invalid scope wire name `{name}`; expected lowercase ASCII kebab-case such as `read-only`"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{expand, expand_with_path, wire_name};

    fn expansion_error(input: proc_macro2::TokenStream) -> String {
        expand(syn::parse2(input).unwrap()).unwrap_err().to_string()
    }

    #[test]
    fn rejects_non_enum_inputs() {
        assert_eq!(
            expansion_error(quote::quote!(
                struct NotAnEnum;
            )),
            "`ScopeRepr` can only be derived for enums"
        );
    }

    #[test]
    fn rejects_empty_closed_domains() {
        assert_eq!(
            expansion_error(quote::quote!(
                enum EmptyScope {}
            )),
            "`ScopeRepr` requires at least one variant; an empty closed scope domain is meaningless"
        );
    }

    #[test]
    fn rejects_non_unit_variants() {
        assert_eq!(
            expansion_error(quote::quote!(
                enum Scope {
                    Unit,
                    Tuple(u8),
                }
            )),
            "`ScopeRepr` requires unit variants; variant `Tuple` has fields"
        );
    }

    #[test]
    fn rejects_duplicate_wire_names_after_renaming() {
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope {
                    Current,
                    #[scope(rename = "current")]
                    Legacy,
                }
            }),
            "duplicate scope wire name `current` after renaming"
        );
    }

    #[test]
    fn rejects_empty_and_invalid_wire_names() {
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope { #[scope(rename = "")] Empty }
            }),
            "scope wire name cannot be empty"
        );
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope { #[scope(rename = "Not_Kebab")] Invalid }
            }),
            "invalid scope wire name `Not_Kebab`; expected lowercase ASCII kebab-case such as `read-only`"
        );
    }

    #[test]
    fn rejects_unknown_and_duplicate_scope_attributes() {
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope { #[scope()] Current }
            }),
            "`scope` attribute requires `rename = \"...\"`"
        );
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope { #[scope(alias = "current")] Current }
            }),
            "unknown `scope` attribute `alias`; expected `rename`"
        );
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope {
                    #[scope(rename = "current")]
                    #[scope(rename = "legacy")]
                    Current,
                }
            }),
            "duplicate `scope` attribute on variant `Current`"
        );
        assert_eq!(
            expansion_error(quote::quote! {
                enum Scope {
                    #[scope(rename = "current", rename = "legacy")]
                    Current,
                }
            }),
            "duplicate `rename` scope attribute"
        );
    }

    #[test]
    fn rejects_scope_attributes_on_the_enum() {
        assert_eq!(
            expansion_error(quote::quote! {
                #[scope(rename = "scope")]
                enum Scope { Current }
            }),
            "`scope` attributes are only supported on enum variants"
        );
    }

    #[test]
    fn generated_code_matches_the_expansion_snapshot() {
        let input: syn::DeriveInput = syn::parse_quote! {
            enum SessionScope {
                All,
                #[scope(rename = "caller-current")]
                Current,
            }
        };
        let variants = match &input.data {
            syn::Data::Enum(data) => data
                .variants
                .iter()
                .map(|variant| {
                    let (name, _) = wire_name(variant).unwrap();
                    (&variant.ident, name)
                })
                .collect::<Vec<_>>(),
            _ => unreachable!(),
        };
        let expansion =
            expand_with_path(&input, &variants, &quote::quote!(::lockgate_policy)).unwrap();
        let expansion = prettyplease::unparse(&syn::parse2(expansion).unwrap());

        assert_eq!(
            expansion,
            include_str!("snapshots/scope_repr_expansion.snap")
        );
    }
}
