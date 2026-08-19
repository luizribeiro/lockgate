use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, GenericArgument, Item, Lit, LitStr, PathArguments, Type, Visibility};

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

    let lockgate_policy = super::lockgate_policy_path(&module.ident)?;
    expand_with_path(capability_id, module, &lockgate_policy)
}

fn expand_with_path(
    capability_id: LitStr,
    mut module: syn::ItemMod,
    lockgate_policy: &TokenStream,
) -> syn::Result<TokenStream> {
    let (_, items) = module
        .content
        .as_mut()
        .expect("inline module was validated");
    let mut permission_ids = BTreeMap::new();
    for item in items {
        let Item::Const(item) = item else { continue };
        if !matches!(item.vis, Visibility::Public(_)) {
            continue;
        }

        let Some(kind) = permission_kind(&item.ty)? else {
            continue;
        };
        let (constructor, permission_id) = declaration_initializer(&item.expr).ok_or_else(|| {
            syn::Error::new_spanned(
                &item.expr,
                "permission declarations must directly call `Permission::new(\"id\")` or `ScopedPermission::new(\"id\")` with a string literal",
            )
        })?;
        if constructor != kind {
            return Err(syn::Error::new_spanned(
                &item.expr,
                format!(
                    "permission const `{}` declares `{}` but initializes `{}`",
                    item.ident,
                    kind.type_name(),
                    constructor.type_name()
                ),
            ));
        }
        validate_id(&permission_id, "permission")?;
        if permission_ids
            .insert(permission_id.value(), permission_id.span())
            .is_some()
        {
            return Err(syn::Error::new(
                permission_id.span(),
                format!(
                    "permission ID `{}` is declared more than once in this capability; permission IDs must be unique",
                    permission_id.value()
                ),
            ));
        }

        let declaration = item.expr.clone();
        let qualifier = kind.qualifier();
        *item.expr = syn::parse_quote_spanned! { permission_id.span() =>
            #lockgate_policy::__private::#qualifier(#capability_id, #declaration)
        };
    }

    Ok(quote!(#module))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionKind {
    Unscoped,
    Scoped,
}

impl PermissionKind {
    fn qualifier(self) -> syn::Ident {
        match self {
            Self::Unscoped => quote::format_ident!("qualify_permission"),
            Self::Scoped => quote::format_ident!("qualify_scoped_permission"),
        }
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::Unscoped => "Permission",
            Self::Scoped => "ScopedPermission<_>",
        }
    }
}

fn permission_kind(ty: &Type) -> syn::Result<Option<PermissionKind>> {
    let ty = peel_type(ty);
    let Type::Path(ty) = ty else { return Ok(None) };
    if ty.qself.is_some() {
        return Ok(None);
    }
    let Some(segment) = ty.path.segments.last() else {
        return Ok(None);
    };
    if segment.ident == "Permission" {
        return match segment.arguments {
            PathArguments::None => Ok(Some(PermissionKind::Unscoped)),
            _ => Err(syn::Error::new_spanned(
                ty,
                "`Permission` does not take generic arguments",
            )),
        };
    }
    if segment.ident != "ScopedPermission" {
        return Ok(None);
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            ty,
            "`ScopedPermission` must directly name one scope type, such as `ScopedPermission<PoolScope>`",
        ));
    };
    if arguments.args.len() != 1
        || !matches!(arguments.args.first(), Some(GenericArgument::Type(_)))
    {
        return Err(syn::Error::new_spanned(
            ty,
            "`ScopedPermission` must directly name exactly one scope type",
        ));
    }
    Ok(Some(PermissionKind::Scoped))
}

fn peel_type(ty: &Type) -> &Type {
    match ty {
        Type::Group(group) => peel_type(&group.elem),
        Type::Paren(paren) => peel_type(&paren.elem),
        ty => ty,
    }
}

fn declaration_initializer(expression: &Expr) -> Option<(PermissionKind, LitStr)> {
    let Expr::Call(call) = peel_expression(expression) else {
        return None;
    };
    let Expr::Path(function) = peel_expression(&call.func) else {
        return None;
    };
    let mut segments = function.path.segments.iter().rev();
    if segments.next()?.ident != "new" {
        return None;
    }
    let kind = match segments.next()?.ident.to_string().as_str() {
        "Permission" => PermissionKind::Unscoped,
        "ScopedPermission" => PermissionKind::Scoped,
        _ => return None,
    };
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(literal) = peel_expression(call.args.first()?) else {
        return None;
    };
    let Lit::Str(permission_id) = &literal.lit else {
        return None;
    };
    Some((kind, permission_id.clone()))
}

fn peel_expression(expression: &Expr) -> &Expr {
    match expression {
        Expr::Group(group) => peel_expression(&group.expr),
        Expr::Paren(paren) => peel_expression(&paren.expr),
        expression => expression,
    }
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
    use super::{expand, expand_with_path};

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

    #[test]
    fn generated_rewrite_matches_the_expansion_snapshot() {
        let module = syn::parse_quote! {
            /// Capability docs.
            pub mod vm {
                use crate::{Permission, ScopedPermission};
                pub const EXEC: crate::ScopedPermission<Instance> =
                    crate::ScopedPermission::new("exec");
                #[allow(dead_code)]
                /// List visible pools.
                pub const LIST: Permission = Permission::new("list");
                pub(crate) const INTERNAL: Permission = Permission::new("internal");
                const PRIVATE: usize = 1;
            }
        };
        let expansion = expand_with_path(
            syn::parse_quote!("vm"),
            module,
            &quote::quote!(::lockgate_policy),
        )
        .unwrap();
        let expansion = prettyplease::unparse(&syn::parse2(expansion).unwrap());

        assert_eq!(
            expansion,
            include_str!("snapshots/capability_expansion.snap")
        );
    }

    #[test]
    fn rejects_duplicates_and_malformed_permission_ids() {
        let cases = [
            (
                quote::quote! {
                    pub mod vm {
                        pub const READ: Permission = Permission::new("read");
                        pub const READ_AGAIN: Permission = Permission::new("read");
                    }
                },
                "permission ID `read` is declared more than once",
            ),
            (
                quote::quote! {
                    pub mod vm {
                        pub const READ: Permission = Permission::new("Read_All");
                    }
                },
                "invalid permission ID `Read_All`",
            ),
            (
                quote::quote! {
                    pub mod vm {
                        pub const READ: Permission = permission_declaration();
                    }
                },
                "permission declarations must directly call",
            ),
            (
                quote::quote! {
                    pub mod vm {
                        pub const READ: Permission = ScopedPermission::new("read");
                    }
                },
                "declares `Permission` but initializes `ScopedPermission<_>`",
            ),
        ];
        for (module, expected) in cases {
            let error = expand_with_path(
                syn::parse_quote!("vm"),
                syn::parse2(module).unwrap(),
                &quote::quote!(::lockgate_policy),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(expected), "unexpected diagnostic: {error}");
        }
    }

    #[test]
    fn accepts_parenthesized_literal_initializers() {
        let module = syn::parse_quote! {
            pub mod vm {
                pub const READ: (Permission) = Permission::new(("read"));
            }
        };

        expand_with_path(
            syn::parse_quote!("vm"),
            module,
            &quote::quote!(::lockgate_policy),
        )
        .unwrap();
    }
}
