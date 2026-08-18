use proc_macro2::TokenStream;
use quote::quote;
use syn::{Item, LitStr};

pub(super) fn expand(capability_id: LitStr, item: Item) -> syn::Result<TokenStream> {
    let Item::Mod(module) = item else {
        return Err(syn::Error::new_spanned(
            item,
            "`#[lockgate::capability]` can only annotate an inline Rust module",
        ));
    };
    if module.content.is_none() {
        return Err(syn::Error::new_spanned(
            &module,
            "`#[lockgate::capability]` requires an inline module; replace `mod name;` with `mod name { ... }`",
        ));
    }
    validate_id(&capability_id, "capability")?;

    Ok(quote!(#module))
}

fn validate_id(id: &LitStr, kind: &str) -> syn::Result<()> {
    let value = id.value();
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes[0] != b'-'
        && bytes.last() != Some(&b'-')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !bytes.windows(2).any(|pair| pair == b"--");
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            id.span(),
            format!(
                "invalid {kind} ID `{value}`; expected lowercase ASCII kebab-case such as `virtual-machines`"
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::expand;

    fn expansion_error(
        capability_id: proc_macro2::TokenStream,
        module: proc_macro2::TokenStream,
    ) -> String {
        expand(
            syn::parse2(capability_id).unwrap(),
            syn::parse2(module).unwrap(),
        )
        .unwrap_err()
        .to_string()
    }

    #[test]
    fn rejects_non_inline_modules_with_a_teaching_diagnostic() {
        assert_eq!(
            expansion_error(
                quote::quote!("vm"),
                quote::quote!(
                    pub mod vm;
                )
            ),
            "`#[lockgate::capability]` requires an inline module; replace `mod name;` with `mod name { ... }`"
        );
    }

    #[test]
    fn rejects_non_module_items_with_a_teaching_diagnostic() {
        assert_eq!(
            expansion_error(
                quote::quote!("vm"),
                quote::quote!(
                    pub struct Vm;
                )
            ),
            "`#[lockgate::capability]` can only annotate an inline Rust module"
        );
    }

    #[test]
    fn rejects_every_malformed_capability_id_shape() {
        for id in [
            "",
            "VM",
            "virtual_machines",
            "-vm",
            "vm-",
            "virtual--machines",
            "é",
        ] {
            assert!(
                expansion_error(
                    quote::quote!(#id),
                    quote::quote!(
                        pub mod vm {}
                    )
                )
                .contains("expected lowercase ASCII kebab-case"),
                "accepted or misdiagnosed {id:?}"
            );
        }
    }
}
