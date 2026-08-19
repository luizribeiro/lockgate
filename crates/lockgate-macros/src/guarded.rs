use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Attribute, Expr, FnArg, Ident, ImplItem, ItemImpl, LitStr, Pat, Path, Token, Type,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
};

pub(super) fn expand(
    mut implementation: ItemImpl,
    lockgate: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let Some((_, trait_path, _)) = &implementation.trait_ else {
        return Err(syn::Error::new_spanned(
            &implementation.self_ty,
            "`#[lockgate::guarded]` requires an impl of a generated host-import `Host` trait",
        ));
    };
    let Some(trait_name) = trait_path.segments.last().map(|segment| &segment.ident) else {
        unreachable!("a Rust path always has at least one segment")
    };
    let trait_name = trait_name.to_string();
    let Some(resource_name) = trait_name.strip_prefix("Host") else {
        return Err(syn::Error::new_spanned(
            trait_path,
            "`#[lockgate::guarded]` requires a generated host-import trait path ending in `Host` or `Host<Resource>`",
        ));
    };
    let mut binding = trait_path.clone();
    binding.segments.last_mut().expect("checked above").ident = if resource_name.is_empty() {
        Ident::new("__LockgateBinding", Span::call_site())
    } else {
        Ident::new(
            &format!("__Lockgate{resource_name}Binding"),
            Span::call_site(),
        )
    };

    let mut policy_methods = Vec::new();
    for item in &mut implementation.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let classification = take_classification(&mut method.attrs, &method.sig.ident)?;
        let method_identity = super::method_identity_const_name(&method.sig.ident);
        let (entry, guard) = match classification {
            Classification::Requires { permission, target } => match target {
                Some(target) => {
                    let target_kind = validate_target(&target, &method.sig.inputs)?;
                    let context = context_parameter(&method.sig.inputs, &method.sig.ident)?;
                    let identifiers = GuardIdentifiers::new(&method.sig.inputs);
                    let subject = &identifiers.subject;
                    let resolve_context = &identifiers.resolve_context;
                    let target_binding = &identifiers.target;
                    let resource = &identifiers.resource;
                    let resolution = match target_kind {
                        TargetKind::Argument => quote! {
                            let #subject = #context.subject();
                            let #target_binding = &(#target);
                            let #resource =
                                #lockgate::__private::resolve_scoped_resource(
                                    &*self,
                                    &#subject,
                                    #target_binding,
                                    #permission,
                                )
                                .await?;
                        },
                        TargetKind::ResourceHandle => quote! {
                            let #resolve_context = #context.resolve_context();
                            let #target_binding = &(#target);
                            let #resource =
                                #lockgate::__private::resolve_scoped_resource_handle(
                                    &*self,
                                    &#resolve_context,
                                    #target_binding,
                                    #permission,
                                )
                                .await?;
                        },
                    };
                    (
                        quote! {
                            #lockgate::__private::PolicyMethod::__requires_scoped(
                                #binding::INTERFACE,
                                #binding::#method_identity,
                                #permission,
                                ::core::stringify!(#target),
                            )
                        },
                        Some(quote! {
                            #resolution
                            #context.require_scoped(#permission, &#resource)?;
                        }),
                    )
                }
                None => {
                    let context = context_parameter(&method.sig.inputs, &method.sig.ident)?;
                    (
                        quote! {
                            #lockgate::__private::PolicyMethod::__requires_unscoped(
                                #binding::INTERFACE,
                                #binding::#method_identity,
                                #permission,
                            )
                        },
                        Some(quote! {
                            #context.require(#permission)?;
                        }),
                    )
                }
            },
            Classification::NoCapabilityRequired { reason } => (
                quote! {
                    #lockgate::__private::PolicyMethod::__no_capability_required(
                        #binding::INTERFACE,
                        #binding::#method_identity,
                        #reason,
                    )
                },
                None,
            ),
        };
        if let Some(guard) = guard {
            let body = &method.block;
            method.block = syn::parse2(quote!({
                #guard
                #body
            }))?;
        }
        policy_methods.push(entry);
    }

    implementation.items.push(syn::parse_quote! {
        const __LOCKGATE_POLICY_METHODS: &'static [#lockgate::__private::PolicyMethod] = &[
            #(#policy_methods),*
        ];
    });
    Ok(quote!(#implementation))
}

struct GuardIdentifiers {
    subject: Ident,
    resolve_context: Ident,
    target: Ident,
    resource: Ident,
}

impl GuardIdentifiers {
    fn new(inputs: &Punctuated<FnArg, Token![,]>) -> Self {
        let mut bindings = PatternBindings::default();
        for input in inputs {
            if let FnArg::Typed(input) = input {
                bindings.visit_pat(&input.pat);
            }
        }
        let subject = fresh_identifier("__lockgate_subject", &mut bindings.names);
        let resolve_context = fresh_identifier("__lockgate_resolve_context", &mut bindings.names);
        let target = fresh_identifier("__lockgate_target", &mut bindings.names);
        let resource = fresh_identifier("__lockgate_resource", &mut bindings.names);
        Self {
            subject,
            resolve_context,
            target,
            resource,
        }
    }
}

fn fresh_identifier(base: &str, bindings: &mut BTreeSet<String>) -> Ident {
    let mut suffix = 0;
    loop {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base}_{suffix}")
        };
        if bindings.insert(candidate.clone()) {
            return Ident::new(&candidate, Span::mixed_site());
        }
        suffix += 1;
    }
}

fn context_parameter(inputs: &Punctuated<FnArg, Token![,]>, method: &Ident) -> syn::Result<Ident> {
    let mut context = None;
    for input in inputs {
        let FnArg::Typed(input) = input else {
            continue;
        };
        if !is_host_context(&input.ty) {
            continue;
        }
        let Pat::Ident(pattern) = input.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &input.pat,
                format!(
                    "host method `{method}` must bind its `HostCtx` parameter to one identifier"
                ),
            ));
        };
        if context.is_some() {
            return Err(syn::Error::new_spanned(
                &input.ty,
                format!("host method `{method}` has more than one `HostCtx` parameter"),
            ));
        }
        context = Some(pattern.ident.clone());
    }
    context.ok_or_else(|| {
        syn::Error::new_spanned(
            method,
            format!("host method `{method}` must have a generated `HostCtx` parameter"),
        )
    })
}

enum Classification {
    Requires {
        permission: Path,
        target: Option<Box<Expr>>,
    },
    NoCapabilityRequired {
        reason: LitStr,
    },
}

fn take_classification(
    attributes: &mut Vec<Attribute>,
    method: &Ident,
) -> syn::Result<Classification> {
    let mut classification = None;
    let mut retained = Vec::with_capacity(attributes.len());
    for attribute in attributes.drain(..) {
        let name = attribute
            .path()
            .segments
            .last()
            .map(|segment| &segment.ident);
        let parsed = match name.map(Ident::to_string).as_deref() {
            Some("requires") => Some(parse_requires(&attribute)?),
            Some("no_capability_required") => Some(parse_no_capability(&attribute)?),
            _ => None,
        };
        let Some(parsed) = parsed else {
            retained.push(attribute);
            continue;
        };
        if classification.is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                format!(
                    "host method `{method}` has more than one policy classification; keep exactly one `requires` or `no_capability_required` attribute"
                ),
            ));
        }
        classification = Some(parsed);
    }
    *attributes = retained;
    classification.ok_or_else(|| {
        syn::Error::new_spanned(
            method,
            format!(
                "host method `{method}` is missing a policy classification; add `#[lockgate::requires(...)]` or `#[lockgate::no_capability_required(reason = \"...\")]`"
            ),
        )
    })
}

fn parse_requires(attribute: &Attribute) -> syn::Result<Classification> {
    let arguments = attribute.parse_args::<RequiresArguments>()?;
    Ok(Classification::Requires {
        permission: arguments.permission,
        target: arguments.target,
    })
}

struct RequiresArguments {
    permission: Path,
    target: Option<Box<Expr>>,
}

impl Parse for RequiresArguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut permission = None;
        let mut target = None;
        while !input.is_empty() {
            let key = input.parse::<Ident>()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "permission" if permission.is_none() => permission = Some(input.parse()?),
                "permission" => {
                    return Err(syn::Error::new_spanned(
                        key,
                        "duplicate `permission` argument",
                    ));
                }
                "target" if target.is_none() => target = Some(Box::new(input.parse()?)),
                "target" => {
                    return Err(syn::Error::new_spanned(key, "duplicate `target` argument"));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        key,
                        "unknown `requires` argument; expected `permission` or `target`",
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(Self {
            permission: permission
                .ok_or_else(|| input.error("`requires` needs `permission = <path>`"))?,
            target,
        })
    }
}

fn parse_no_capability(attribute: &Attribute) -> syn::Result<Classification> {
    if matches!(attribute.meta, syn::Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            attribute,
            "`no_capability_required` needs a non-empty `reason = \"...\"`",
        ));
    }
    let arguments =
        attribute.parse_args_with(Punctuated::<ReasonArgument, Token![,]>::parse_terminated)?;
    let mut reason = None;
    for argument in arguments {
        if reason.is_some() {
            return Err(syn::Error::new_spanned(
                argument.key,
                "duplicate `reason` argument",
            ));
        }
        reason = Some(argument.value);
    }
    let reason = reason.ok_or_else(|| {
        syn::Error::new_spanned(
            attribute,
            "`no_capability_required` needs a non-empty `reason = \"...\"`",
        )
    })?;
    if reason.value().trim().is_empty() {
        return Err(syn::Error::new_spanned(
            &reason,
            "`no_capability_required` reason must not be empty or whitespace-only",
        ));
    }
    Ok(Classification::NoCapabilityRequired { reason })
}

struct ReasonArgument {
    key: Ident,
    value: LitStr,
}

impl Parse for ReasonArgument {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key = input.parse::<Ident>()?;
        if key != "reason" {
            return Err(syn::Error::new_spanned(
                key,
                "unknown `no_capability_required` argument; expected `reason`",
            ));
        }
        input.parse::<Token![=]>()?;
        Ok(Self {
            key,
            value: input.parse()?,
        })
    }
}

#[derive(Clone, Copy)]
enum TargetKind {
    Argument,
    ResourceHandle,
}

fn validate_target(
    target: &Expr,
    inputs: &Punctuated<FnArg, Token![,]>,
) -> syn::Result<TargetKind> {
    let mut forbidden = ForbiddenExpression::default();
    forbidden.visit_expr(target);
    if let Some(span) = forbidden.self_span {
        return Err(syn::Error::new(
            span,
            "policy targets cannot contain `self`; use a named method parameter or a path rooted at `HostCtx::data()`",
        ));
    }
    if let Some(span) = forbidden.control_flow_span {
        return Err(syn::Error::new(
            span,
            "policy targets cannot contain control flow; use a direct parameter path or `HostCtx::data()` path",
        ));
    }

    let root = target_root(target)?;
    let mut named = BTreeSet::new();
    let mut named_types = BTreeMap::new();
    let mut destructured = BTreeSet::new();
    let mut context = None;
    for input in inputs {
        let FnArg::Typed(input) = input else {
            continue;
        };
        match input.pat.as_ref() {
            Pat::Ident(pattern) if pattern.subpat.is_none() => {
                named.insert(pattern.ident.to_string());
                named_types.insert(pattern.ident.to_string(), input.ty.as_ref());
                if is_host_context(&input.ty) {
                    context = Some(pattern.ident.to_string());
                }
            }
            pattern => {
                let mut bindings = PatternBindings::default();
                bindings.visit_pat(pattern);
                destructured.extend(bindings.names);
            }
        }
    }

    let root_name = root.ident().to_string();
    if destructured.contains(&root_name) {
        return Err(syn::Error::new(
            root.ident().span(),
            format!(
                "policy target `{root_name}` comes from a destructured parameter; give that parameter one identifier and target a path from it"
            ),
        ));
    }
    if !named.contains(&root_name) {
        return Err(syn::Error::new(
            root.ident().span(),
            format!("policy target root `{root_name}` is not a named method parameter"),
        ));
    }
    if matches!(root, TargetRoot::ContextData(_)) && context.as_deref() != Some(&root_name) {
        return Err(syn::Error::new(
            root.ident().span(),
            "a `.data()` policy target must be rooted at this method's `HostCtx` parameter",
        ));
    }
    let resource_handle = matches!(root, TargetRoot::Parameter(_))
        && named_types
            .get(&root_name)
            .is_some_and(|ty| is_resource_handle(ty));
    if resource_handle
        && !matches!(
            target,
            Expr::Path(path)
                if path.qself.is_none()
                    && path.path.leading_colon.is_none()
                    && path.path.segments.len() == 1
        )
    {
        return Err(syn::Error::new_spanned(
            target,
            "a WIT resource-handle policy target must be the handle parameter itself, not a path through it",
        ));
    }
    Ok(if resource_handle {
        TargetKind::ResourceHandle
    } else {
        TargetKind::Argument
    })
}

fn is_resource_handle(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Resource"),
        Type::Paren(paren) => is_resource_handle(&paren.elem),
        Type::Group(group) => is_resource_handle(&group.elem),
        _ => false,
    }
}

enum TargetRoot<'a> {
    Parameter(&'a Ident),
    ContextData(&'a Ident),
}

impl TargetRoot<'_> {
    fn ident(&self) -> &Ident {
        match self {
            Self::Parameter(ident) | Self::ContextData(ident) => ident,
        }
    }
}

fn target_root(expression: &Expr) -> syn::Result<TargetRoot<'_>> {
    match expression {
        Expr::Path(path)
            if path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1 =>
        {
            Ok(TargetRoot::Parameter(&path.path.segments[0].ident))
        }
        Expr::Field(field) => target_root(&field.base),
        Expr::Paren(paren) => target_root(&paren.expr),
        Expr::Group(group) => target_root(&group.expr),
        Expr::MethodCall(call)
            if call.method == "data"
                && call.args.is_empty()
                && call.turbofish.is_none()
                && matches!(call.receiver.as_ref(), Expr::Path(_)) =>
        {
            let TargetRoot::Parameter(ident) = target_root(&call.receiver)? else {
                unreachable!("a path receiver produces a parameter root")
            };
            Ok(TargetRoot::ContextData(ident))
        }
        _ => Err(syn::Error::new_spanned(
            expression,
            "unsupported policy target; use a path rooted at a named method parameter (for example `request.id`) or at `HostCtx::data()` (for example `cx.data().session`)",
        )),
    }
}

fn is_host_context(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "HostCtx"),
        Type::Reference(reference) => is_host_context(&reference.elem),
        Type::Paren(paren) => is_host_context(&paren.elem),
        Type::Group(group) => is_host_context(&group.elem),
        _ => false,
    }
}

#[derive(Default)]
struct ForbiddenExpression {
    self_span: Option<Span>,
    control_flow_span: Option<Span>,
}

impl<'ast> Visit<'ast> for ForbiddenExpression {
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if path.path.is_ident("self") && self.self_span.is_none() {
            self.self_span = Some(path.span());
        }
        visit::visit_expr_path(self, path);
    }

    fn visit_expr(&mut self, expression: &'ast Expr) {
        if self.control_flow_span.is_none()
            && matches!(
                expression,
                Expr::Async(_)
                    | Expr::Block(_)
                    | Expr::Break(_)
                    | Expr::Closure(_)
                    | Expr::Continue(_)
                    | Expr::ForLoop(_)
                    | Expr::If(_)
                    | Expr::Let(_)
                    | Expr::Loop(_)
                    | Expr::Match(_)
                    | Expr::Return(_)
                    | Expr::TryBlock(_)
                    | Expr::While(_)
            )
        {
            self.control_flow_span = Some(expression.span());
        }
        visit::visit_expr(self, expression);
    }
}

#[derive(Default)]
struct PatternBindings {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for PatternBindings {
    fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
        self.names.insert(pattern.ident.to_string());
        visit::visit_pat_ident(self, pattern);
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    #[test]
    fn guarded_impl_matches_the_expansion_snapshot() {
        let implementation = syn::parse_quote! {
            impl vm::Host for Imports {
                #[lockgate::requires(permission = permissions::EXEC, target = request.id)]
                async fn exec(
                    &mut self,
                    cx: lockgate::HostCtx<'_, Data>,
                    request: Request,
                ) -> Result<(), Error> {
                    self.execute(cx, request).await
                }

                #[lockgate::requires(permission = permissions::LIST)]
                async fn list(&mut self, cx: lockgate::HostCtx<'_, Data>) -> Result<(), Error> {
                    self.list(cx).await
                }

                #[lockgate::no_capability_required(reason = "static protocol version")]
                async fn version(&mut self, _cx: lockgate::HostCtx<'_, Data>) -> String {
                    "1".to_owned()
                }
            }
        };
        let expansion = super::expand(implementation, &quote!(::lockgate)).unwrap();
        let expansion = prettyplease::unparse(&syn::parse2(expansion).unwrap());

        assert_eq!(expansion, include_str!("snapshots/guarded_expansion.snap"));
    }

    #[test]
    fn call_context_guard_matches_the_expansion_snapshot() {
        let implementation = syn::parse_quote! {
            impl sessions::Host for Imports {
                #[lockgate::requires(
                    permission = permissions::READ,
                    target = cx.data().session
                )]
                async fn read(
                    &mut self,
                    cx: lockgate::HostCtx<'_, SageCall>,
                ) -> Result<(), Error> {
                    self.read_session(cx).await
                }
            }
        };
        let expansion = super::expand(implementation, &quote!(::lockgate)).unwrap();
        let expansion = prettyplease::unparse(&syn::parse2(expansion).unwrap());

        assert_eq!(
            expansion,
            include_str!("snapshots/call_context_guarded_expansion.snap")
        );
    }

    #[test]
    fn resource_guard_matches_the_expansion_snapshot() {
        let implementation = syn::parse_quote! {
            impl sessions::HostSession for Imports {
                #[lockgate::requires(permission = permissions::SEND, target = session)]
                async fn send(
                    &mut self,
                    cx: lockgate::HostCtx<'_, Data>,
                    session: lockgate::Resource<Session>,
                    message: String,
                ) -> Result<(), Error> {
                    self.send_message(cx, session, message).await
                }
            }
        };
        let expansion = super::expand(implementation, &quote!(::lockgate)).unwrap();
        let expansion = prettyplease::unparse(&syn::parse2(expansion).unwrap());

        assert_eq!(
            expansion,
            include_str!("snapshots/resource_guarded_expansion.snap")
        );
    }
}
