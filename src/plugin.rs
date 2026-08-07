//! Structural WIT and Wasmtime component type validation shared by discovery and planning.
//! The helpers reject values that cannot safely cross independently owned component stores.

use anyhow::{Context, Result, bail};
use std::fmt;
use wasmtime::{
    Engine,
    component::{
        Component,
        types::{ComponentFunc, ComponentItem, Type},
    },
};
use wit_parser::{Resolve, Type as WitType, TypeDefKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) params: Vec<Type>,
    pub(crate) results: Vec<Type>,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectImport {
    pub(crate) interface: String,
    pub(crate) functions: Vec<(String, Signature)>,
}

impl fmt::Display for Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params = self
            .params
            .iter()
            .map(type_name)
            .collect::<Vec<_>>()
            .join(", ");
        let results = match self.results.as_slice() {
            [] => "()".into(),
            [result] => type_name(result),
            results => format!(
                "({})",
                results.iter().map(type_name).collect::<Vec<_>>().join(", ")
            ),
        };
        write!(formatter, "({}) -> {}", params, results)
    }
}

pub(crate) fn validate_wit_interface(
    resolve: &Resolve,
    interface: wit_parser::InterfaceId,
) -> Result<()> {
    for function in resolve.interfaces[interface].functions.values() {
        if function.kind.is_async() {
            bail!("async plugin functions are unsupported");
        }
        if function.kind.resource().is_some() {
            bail!("resource methods are unsupported");
        }
        for param in &function.params {
            validate_wit_type(resolve, param.ty)?;
        }
        if let Some(result) = function.result {
            validate_wit_type(resolve, result)?;
        }
    }
    Ok(())
}

fn validate_wit_type(resolve: &Resolve, ty: WitType) -> Result<()> {
    let WitType::Id(id) = ty else {
        if ty == WitType::ErrorContext {
            bail!("error-context values cannot cross plugin stores");
        }
        return Ok(());
    };
    match &resolve.types[id].kind {
        TypeDefKind::Record(record) => record
            .fields
            .iter()
            .try_for_each(|field| validate_wit_type(resolve, field.ty)),
        TypeDefKind::Tuple(tuple) => tuple
            .types
            .iter()
            .try_for_each(|ty| validate_wit_type(resolve, *ty)),
        TypeDefKind::Variant(variant) => variant
            .cases
            .iter()
            .filter_map(|case| case.ty)
            .try_for_each(|ty| validate_wit_type(resolve, ty)),
        TypeDefKind::Option(ty) | TypeDefKind::List(ty) | TypeDefKind::Type(ty) => {
            validate_wit_type(resolve, *ty)
        }
        TypeDefKind::Result(result) => result
            .ok
            .into_iter()
            .chain(result.err)
            .try_for_each(|ty| validate_wit_type(resolve, ty)),
        TypeDefKind::Map(key, value) => {
            validate_wit_type(resolve, *key)?;
            validate_wit_type(resolve, *value)
        }
        TypeDefKind::Resource | TypeDefKind::Handle(_) => {
            bail!("resource handles are unsupported")
        }
        TypeDefKind::Future(_) | TypeDefKind::Stream(_) => {
            bail!("async value types are unsupported")
        }
        TypeDefKind::FixedLengthList(_, _) => {
            bail!("fixed-length lists are unsupported by Wasmtime 47's component type API")
        }
        TypeDefKind::Flags(_) | TypeDefKind::Enum(_) => Ok(()),
        TypeDefKind::Unknown => unreachable!("resolved WIT cannot contain unknown types"),
    }
}

pub(crate) fn interface_functions(
    engine: &Engine,
    component: &Component,
    interface: &str,
    imported: bool,
) -> Result<Vec<(String, Signature)>> {
    let component_type = component.component_type();
    let item = if imported {
        component_type.get_import(engine, interface)
    } else {
        component_type.get_export(engine, interface)
    }
    .with_context(|| format!("component type is missing interface {interface}"))?;
    let ComponentItem::ComponentInstance(instance) = item.ty else {
        bail!("component item {interface} is not an interface");
    };
    instance
        .exports(engine)
        .filter_map(|(name, item)| match item.ty {
            ComponentItem::ComponentFunc(function) => Some(
                Signature::from_component(&function).map(|signature| (name.to_string(), signature)),
            ),
            ComponentItem::Type(_) | ComponentItem::Resource(_) => None,
            _ => Some(Err(anyhow::anyhow!(
                "unsupported item {interface}#{name} in plugin interface"
            ))),
        })
        .collect()
}

impl Signature {
    fn from_component(function: &ComponentFunc) -> Result<Self> {
        if function.async_() {
            bail!("async plugin functions are unsupported");
        }
        let signature = Self {
            params: function.params().map(|(_, ty)| ty).collect(),
            results: function.results().collect(),
        };
        for ty in signature.params.iter().chain(&signature.results) {
            validate_type(ty)?;
        }
        Ok(signature)
    }
}

fn validate_type(ty: &Type) -> Result<()> {
    match ty {
        Type::List(list) => validate_type(&list.ty()),
        Type::Map(map) => {
            validate_type(&map.key())?;
            validate_type(&map.value())
        }
        Type::Record(record) => record
            .fields()
            .try_for_each(|field| validate_type(&field.ty)),
        Type::Tuple(tuple) => tuple.types().try_for_each(|ty| validate_type(&ty)),
        Type::Variant(variant) => variant
            .cases()
            .filter_map(|case| case.ty)
            .try_for_each(|ty| validate_type(&ty)),
        Type::Option(option) => validate_type(&option.ty()),
        Type::Result(result) => result
            .ok()
            .into_iter()
            .chain(result.err())
            .try_for_each(|ty| validate_type(&ty)),
        Type::Own(_) | Type::Borrow(_) => bail!("resource handles are unsupported"),
        Type::Future(_) | Type::Stream(_) => bail!("async value types are unsupported"),
        Type::ErrorContext => bail!("error-context values cannot cross plugin stores"),
        _ => Ok(()),
    }
}

pub(crate) fn type_name(ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::S8 => "s8".into(),
        Type::U8 => "u8".into(),
        Type::S16 => "s16".into(),
        Type::U16 => "u16".into(),
        Type::S32 => "s32".into(),
        Type::U32 => "u32".into(),
        Type::S64 => "s64".into(),
        Type::U64 => "u64".into(),
        Type::Float32 => "f32".into(),
        Type::Float64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::List(list) => format!("list<{}>", type_name(&list.ty())),
        Type::Map(map) => format!(
            "map<{}, {}>",
            type_name(&map.key()),
            type_name(&map.value())
        ),
        Type::Record(record) => format!(
            "record {{ {} }}",
            record
                .fields()
                .map(|field| format!("{}: {}", field.name, type_name(&field.ty)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Tuple(tuple) => format!(
            "tuple<{}>",
            tuple
                .types()
                .map(|ty| type_name(&ty))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Variant(variant) => format!(
            "variant {{ {} }}",
            variant
                .cases()
                .map(|case| match case.ty {
                    Some(ty) => format!("{}({})", case.name, type_name(&ty)),
                    None => case.name.into(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Enum(enumeration) => {
            format!(
                "enum {{ {} }}",
                enumeration.names().collect::<Vec<_>>().join(", ")
            )
        }
        Type::Option(option) => format!("option<{}>", type_name(&option.ty())),
        Type::Result(result) => format!(
            "result<{}, {}>",
            result
                .ok()
                .as_ref()
                .map(type_name)
                .unwrap_or_else(|| "_".into()),
            result
                .err()
                .as_ref()
                .map(type_name)
                .unwrap_or_else(|| "_".into())
        ),
        Type::Flags(flags) => {
            format!(
                "flags {{ {} }}",
                flags.names().collect::<Vec<_>>().join(", ")
            )
        }
        Type::Own(_) => "own<resource>".into(),
        Type::Borrow(_) => "borrow<resource>".into(),
        Type::Future(future) => format!(
            "future<{}>",
            future
                .ty()
                .as_ref()
                .map(type_name)
                .unwrap_or_else(|| "_".into())
        ),
        Type::Stream(stream) => format!(
            "stream<{}>",
            stream
                .ty()
                .as_ref()
                .map(type_name)
                .unwrap_or_else(|| "_".into())
        ),
        Type::ErrorContext => "error-context".into(),
    }
}
