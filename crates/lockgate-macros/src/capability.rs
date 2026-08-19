use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, GenericArgument, Item, ItemConst, Lit, LitStr, Meta, PathArguments, Type, Visibility,
    ext::IdentExt, parse::Parser, punctuated::Punctuated, spanned::Spanned,
};

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
    let mut permission_aliases = BTreeSet::new();
    let mut alias_targets = BTreeMap::new();
    for item in items.iter() {
        if let Item::Type(alias) = item {
            if matches!(permission_kind(&alias.ty), Ok(Some(_)))
                || is_projected_permission_alias(&alias.ty)
            {
                permission_aliases.insert(alias.ident.to_string());
            } else if let Some(target) = local_type_alias_ident(&alias.ty) {
                alias_targets.insert(alias.ident.to_string(), target.to_string());
            }
        }
    }
    loop {
        let newly_discovered = alias_targets
            .iter()
            .filter(|(alias, target)| {
                !permission_aliases.contains(*alias) && permission_aliases.contains(*target)
            })
            .map(|(alias, _)| alias.clone())
            .collect::<Vec<_>>();
        if newly_discovered.is_empty() {
            break;
        }
        permission_aliases.extend(newly_discovered);
    }
    let mut permission_ids = BTreeMap::<String, Vec<Vec<syn::Attribute>>>::new();
    let mut conditional_duplicate_errors = Vec::new();
    let mut permissions = Vec::new();
    for item in items.iter_mut() {
        let Item::Const(item) = item else { continue };
        if !matches!(item.vis, Visibility::Public(_)) {
            continue;
        }

        let Some(kind) = permission_kind(&item.ty)? else {
            reject_likely_alias(item, &permission_aliases)?;
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
        let conditionals = conditional_attributes(&item.attrs)?;
        let previous_declarations = permission_ids.entry(permission_id.value()).or_default();
        for previous_conditionals in previous_declarations.iter() {
            let message = format!(
                "permission ID `{}` is declared more than once in this capability; permission IDs must be unique",
                permission_id.value()
            );
            if previous_conditionals.is_empty() && conditionals.is_empty() {
                return Err(syn::Error::new(permission_id.span(), message));
            }
            conditional_duplicate_errors.push(quote::quote_spanned! { permission_id.span() =>
                #(#previous_conditionals)*
                #(#conditionals)*
                ::core::compile_error!(#message);
            });
        }
        previous_declarations.push(conditionals.clone());

        let declaration = item.expr.clone();
        let qualifier = kind.qualifier();
        *item.expr = syn::parse_quote_spanned! { permission_id.span() =>
            #lockgate_policy::__private::#qualifier(#capability_id, #declaration)
        };
        permissions.push((item.ident.clone(), kind, conditionals));
    }

    let descriptors = permissions.iter().map(|(permission, kind, conditionals)| {
        let eraser = kind.eraser();
        quote!(#(#conditionals)* #lockgate_policy::__private::#eraser(#permission))
    });
    for item in items.iter() {
        if item_defines_reserved_name(item) {
            return Err(syn::Error::new_spanned(
                item,
                "`#[lockgate::capability]` reserves `Contract` and `__LOCKGATE_CAPABILITY_PERMISSIONS` for generated registration metadata",
            ));
        }
    }
    for duplicate_error in conditional_duplicate_errors {
        items.push(syn::parse2(duplicate_error)?);
    }
    items.push(syn::parse_quote! {
        #[allow(deprecated)]
        static __LOCKGATE_CAPABILITY_PERMISSIONS:
            &'static [#lockgate_policy::__private::ErasedPermission] =
            &[#(#descriptors),*];
    });
    items.push(syn::parse_quote! {
        /// Registration anchor generated for this capability vocabulary.
        pub struct Contract;
    });
    items.push(syn::parse_quote! {
        impl #lockgate_policy::CapabilityContract for Contract {
            const ID: &'static str = #capability_id;

            #[doc(hidden)]
            fn permissions() -> &'static [#lockgate_policy::__private::ErasedPermission] {
                __LOCKGATE_CAPABILITY_PERMISSIONS
            }
        }
    });

    Ok(quote!(#module))
}

fn conditional_attributes(attributes: &[syn::Attribute]) -> syn::Result<Vec<syn::Attribute>> {
    let mut conditionals = Vec::new();
    for attribute in attributes {
        if attribute.path().is_ident("cfg") {
            conditionals.push(attribute.clone());
        } else if attribute.path().is_ident("cfg_attr")
            && let Some(meta) = sanitize_cfg_attr(&attribute.meta)?
        {
            conditionals.push(syn::parse_quote_spanned!(attribute.path().span()=> #[#meta]));
        }
    }
    Ok(conditionals)
}

fn sanitize_cfg_attr(meta: &Meta) -> syn::Result<Option<Meta>> {
    let Meta::List(list) = meta else {
        return Err(syn::Error::new_spanned(
            meta,
            "malformed `cfg_attr`; expected `cfg_attr(condition, attribute)`",
        ));
    };
    let parsed = Punctuated::<Meta, syn::Token![,]>::parse_terminated.parse2(list.tokens.clone());
    let mut metas = parsed?.into_iter();
    let condition = metas.next().ok_or_else(|| {
        syn::Error::new_spanned(meta, "`cfg_attr` requires a condition and an attribute")
    })?;
    let mut nested = Vec::new();
    for meta in metas {
        if meta.path().is_ident("cfg") {
            nested.push(meta);
        } else if meta.path().is_ident("cfg_attr")
            && let Some(meta) = sanitize_cfg_attr(&meta)?
        {
            nested.push(meta);
        }
    }
    if nested.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        syn::parse_quote_spanned!(list.path.span()=> cfg_attr(#condition, #(#nested),*)),
    ))
}

fn item_defines_reserved_name(item: &Item) -> bool {
    let reserved = |ident: &syn::Ident| {
        matches!(
            ident.unraw().to_string().as_str(),
            "Contract" | "__LOCKGATE_CAPABILITY_PERMISSIONS"
        )
    };
    match item {
        Item::Const(item) => reserved(&item.ident),
        Item::Enum(item) => reserved(&item.ident),
        Item::ExternCrate(item) => {
            reserved(item.rename.as_ref().map_or(&item.ident, |(_, name)| name))
        }
        Item::Fn(item) => reserved(&item.sig.ident),
        Item::Macro(item) => item.ident.as_ref().is_some_and(reserved),
        Item::Mod(item) => reserved(&item.ident),
        Item::Static(item) => reserved(&item.ident),
        Item::Struct(item) => reserved(&item.ident),
        Item::Trait(item) => reserved(&item.ident),
        Item::TraitAlias(item) => reserved(&item.ident),
        Item::Type(item) => reserved(&item.ident),
        Item::Union(item) => reserved(&item.ident),
        Item::Use(item) => use_tree_defines_name(&item.tree, &reserved, None),
        _ => false,
    }
}

fn use_tree_defines_name(
    tree: &syn::UseTree,
    reserved: &impl Fn(&syn::Ident) -> bool,
    parent: Option<&syn::Ident>,
) -> bool {
    match tree {
        syn::UseTree::Path(path) => use_tree_defines_name(&path.tree, reserved, Some(&path.ident)),
        syn::UseTree::Name(name) if name.ident == "self" => parent.is_some_and(reserved),
        syn::UseTree::Name(name) => reserved(&name.ident),
        syn::UseTree::Rename(rename) => reserved(&rename.rename),
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .any(|tree| use_tree_defines_name(tree, reserved, parent)),
        syn::UseTree::Glob(_) => false,
    }
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

    fn eraser(self) -> syn::Ident {
        match self {
            Self::Unscoped => quote::format_ident!("erase_permission"),
            Self::Scoped => quote::format_ident!("erase_scoped_permission"),
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

fn reject_likely_alias(item: &ItemConst, permission_aliases: &BTreeSet<String>) -> syn::Result<()> {
    // WHY: Nested macro output, imported aliases (including renamed `Permission`), and
    // associated consts cannot be classified without name resolution or macro expansion.
    // Leaving them untouched preserves ordinary items, while the declaration/handle type
    // split makes each hidden permission fail with E0308; trybuild pins those safety nets.
    let local_alias = local_type_alias_ident(&item.ty)
        .is_some_and(|ident| permission_aliases.contains(&ident.to_string()));
    if local_alias
        || is_projected_permission_alias(&item.ty)
        || declaration_initializer(&item.expr).is_some()
    {
        Err(syn::Error::new_spanned(
            &item.ty,
            format!(
                "permission const `{}` hides its permission type behind an alias; spell `Permission` or `ScopedPermission<Scope>` directly so `#[lockgate::capability]` can discover it",
                item.ident
            ),
        ))
    } else {
        Ok(())
    }
}

fn local_type_alias_ident(ty: &Type) -> Option<&syn::Ident> {
    let Type::Path(ty) = peel_type(ty) else {
        return None;
    };
    if ty.qself.is_some() {
        return None;
    }
    match ty.path.segments.len() {
        1 => Some(&ty.path.segments.first()?.ident),
        2 if ty.path.segments.first()?.ident == "self" => Some(&ty.path.segments.last()?.ident),
        _ => None,
    }
}

fn is_projected_permission_alias(ty: &Type) -> bool {
    let Type::Path(ty) = peel_type(ty) else {
        return false;
    };
    ty.qself.is_some()
        && ty.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "Permission" | "ScopedPermission"
            )
        })
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
    // WHY: `lockgate-policy` depends on this proc-macro crate, so sharing its const
    // validator here would create a cycle. Keep this algorithm and the boundary vectors
    // below in sync with `lockgate-policy/src/atom.rs::is_valid_authoring_id`.
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
    fn authoring_id_boundary_vectors_match_policy() {
        // Keep this literal table verbatim with atom.rs::authoring_id_boundary_vectors_match_macro.
        let vectors = [
            ("", false),
            ("a", true),
            ("-a", false),
            ("a-", false),
            ("a--b", false),
            ("A", false),
            ("a_b", false),
            ("123", true),
            ("é", false),
            ("a-b2", true),
        ];

        for (id, expected_valid) in vectors {
            let literal = syn::LitStr::new(id, proc_macro2::Span::call_site());
            assert_eq!(
                super::validate_id(&literal, "capability").is_ok(),
                expected_valid,
                "unexpected authoring-ID result for {id:?}",
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
    fn rejects_aliases_duplicates_and_malformed_permission_ids() {
        let cases = [
            (
                quote::quote! {
                    pub mod vm {
                        type Alias = Permission;
                        pub const READ: Alias = Alias::new("read");
                    }
                },
                "hides its permission type behind an alias",
            ),
            (
                quote::quote! {
                    pub mod vm {
                        pub const READ: <Marker as Contract>::Permission =
                            make_permission();
                    }
                },
                "hides its permission type behind an alias",
            ),
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
    fn preserves_unrelated_qualified_aliases_and_constructor_consts() {
        let module = syn::parse_quote! {
            pub mod vm {
                type Foreign = other::Permission<u8>;
                type Alias = Permission;
                pub const OTHER: other::Alias = other::Alias::new("other");
            }
        };

        let expansion = expand_with_path(
            syn::parse_quote!("vm"),
            module,
            &quote::quote!(::lockgate_policy),
        )
        .unwrap()
        .to_string();
        assert!(expansion.contains("other :: Permission < u8 >"));
        assert!(expansion.contains("other :: Alias :: new"));
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

    #[test]
    fn rejects_self_qualified_and_chained_aliases_of_qualified_handles() {
        for module in [
            quote::quote! {
                pub mod vm {
                    type Alias = Permission;
                    pub const READ: Permission = Permission::new("read");
                    pub const COPY: self::Alias = READ;
                }
            },
            quote::quote! {
                pub mod vm {
                    type First = Permission;
                    type Alias = First;
                    pub const READ: Permission = Permission::new("read");
                    pub const COPY: Alias = READ;
                }
            },
        ] {
            let error = expand_with_path(
                syn::parse_quote!("vm"),
                syn::parse2(module).unwrap(),
                &quote::quote!(::lockgate_policy),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("hides its permission type behind an alias"));
        }
    }

    #[test]
    fn rejects_grouped_self_imports_of_generated_names() {
        for module in [
            quote::quote! {
                pub mod vm {
                    use crate::Contract::{self};
                }
            },
            quote::quote! {
                pub mod vm {
                    pub struct r#Contract;
                }
            },
        ] {
            let error = expand_with_path(
                syn::parse_quote!("vm"),
                syn::parse2(module).unwrap(),
                &quote::quote!(::lockgate_policy),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("reserves `Contract`"));
        }
    }
}
