use anyhow::{Result, bail};
use wit_parser::{
    Function, FunctionKind, Handle, Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldId,
    WorldItem,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub interface: String,
    pub item: String,
    pub signature: String,
}

pub fn world_exports(resolve: &Resolve, world: WorldId) -> Result<Vec<Export>> {
    let mut exports = Vec::new();
    for item in resolve.worlds[world].exports.values() {
        let WorldItem::Interface { id, .. } = item else {
            bail!("direct world functions and types are unsupported");
        };
        let interface = resolve
            .id_of(*id)
            .ok_or_else(|| anyhow::anyhow!("world exports an unnamed interface"))?;
        for (name, ty) in &resolve.interfaces[*id].types {
            exports.push(Export {
                interface: interface.clone(),
                item: format!("[type]{name}"),
                signature: type_id_name(resolve, *ty)?,
            });
        }
        for function in resolve.interfaces[*id].functions.values() {
            exports.push(Export {
                interface: interface.clone(),
                item: function.name.clone(),
                signature: function_signature(resolve, function)?,
            });
        }
    }
    Ok(exports)
}

fn function_signature(resolve: &Resolve, function: &Function) -> Result<String> {
    let kind = match function.kind {
        FunctionKind::Freestanding => "freestanding".into(),
        FunctionKind::AsyncFreestanding => "async-freestanding".into(),
        FunctionKind::Method(resource) => format!("method<{}>", resource_name(resolve, resource)?),
        FunctionKind::AsyncMethod(resource) => {
            format!("async-method<{}>", resource_name(resolve, resource)?)
        }
        FunctionKind::Static(resource) => format!("static<{}>", resource_name(resolve, resource)?),
        FunctionKind::AsyncStatic(resource) => {
            format!("async-static<{}>", resource_name(resolve, resource)?)
        }
        FunctionKind::Constructor(resource) => {
            format!("constructor<{}>", resource_name(resolve, resource)?)
        }
    };
    let params = function
        .params
        .iter()
        .map(|param| type_name(resolve, param.ty))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    let result = function
        .result
        .map(|ty| type_name(resolve, ty))
        .transpose()?
        .unwrap_or_else(|| "unit".into());
    Ok(format!("{kind}({params})->{result}"))
}

fn type_name(resolve: &Resolve, ty: Type) -> Result<String> {
    Ok(match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => type_id_name(resolve, id)?,
    })
}

fn type_id_name(resolve: &Resolve, id: TypeId) -> Result<String> {
    Ok(match &resolve.types[id].kind {
        TypeDefKind::Type(ty) => type_name(resolve, *ty)?,
        TypeDefKind::Resource => format!("resource<{}>", resource_name(resolve, id)?),
        TypeDefKind::Handle(Handle::Own(resource)) => {
            format!("own<{}>", resource_name(resolve, *resource)?)
        }
        TypeDefKind::Handle(Handle::Borrow(resource)) => {
            format!("borrow<{}>", resource_name(resolve, *resource)?)
        }
        TypeDefKind::Record(record) => format!(
            "record{{{}}}",
            record
                .fields
                .iter()
                .map(|field| Ok(format!("{}:{}", field.name, type_name(resolve, field.ty)?)))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        TypeDefKind::Tuple(tuple) => format!(
            "tuple<{}>",
            tuple
                .types
                .iter()
                .map(|ty| type_name(resolve, *ty))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        TypeDefKind::Variant(variant) => format!(
            "variant{{{}}}",
            variant
                .cases
                .iter()
                .map(|case| match case.ty {
                    Some(ty) => Ok(format!("{}({})", case.name, type_name(resolve, ty)?)),
                    None => Ok(case.name.clone()),
                })
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        TypeDefKind::Enum(enumeration) => format!(
            "enum{{{}}}",
            enumeration
                .cases
                .iter()
                .map(|case| case.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Flags(flags) => format!(
            "flags{{{}}}",
            flags
                .flags
                .iter()
                .map(|flag| flag.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        TypeDefKind::Option(ty) => format!("option<{}>", type_name(resolve, *ty)?),
        TypeDefKind::Result(result) => format!(
            "result<{},{}>",
            optional_type_name(resolve, result.ok)?,
            optional_type_name(resolve, result.err)?
        ),
        TypeDefKind::List(ty) => format!("list<{}>", type_name(resolve, *ty)?),
        TypeDefKind::Map(key, value) => format!(
            "map<{},{}>",
            type_name(resolve, *key)?,
            type_name(resolve, *value)?
        ),
        TypeDefKind::FixedLengthList(ty, length) => {
            format!("list<{};{length}>", type_name(resolve, *ty)?)
        }
        TypeDefKind::Future(ty) => format!("future<{}>", optional_type_name(resolve, *ty)?),
        TypeDefKind::Stream(ty) => format!("stream<{}>", optional_type_name(resolve, *ty)?),
        TypeDefKind::Unknown => bail!("resolved WIT contains an unknown type"),
    })
}

fn optional_type_name(resolve: &Resolve, ty: Option<Type>) -> Result<String> {
    ty.map(|ty| type_name(resolve, ty))
        .transpose()
        .map(|ty| ty.unwrap_or_else(|| "unit".into()))
}

fn resource_name(resolve: &Resolve, id: TypeId) -> Result<String> {
    let definition = &resolve.types[id];
    if let TypeDefKind::Type(Type::Id(alias)) = definition.kind {
        return resource_name(resolve, alias);
    }
    let name = definition
        .name
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("resource type is unnamed"))?;
    let owner = match definition.owner {
        TypeOwner::Interface(interface) => resolve
            .id_of(interface)
            .ok_or_else(|| anyhow::anyhow!("resource belongs to an unnamed interface"))?,
        TypeOwner::World(world) => {
            let world = &resolve.worlds[world];
            match world.package {
                Some(package) => format!("{}/{}", resolve.packages[package].name, world.name),
                None => world.name.clone(),
            }
        }
        TypeOwner::None => "anonymous".into(),
    };
    Ok(format!("{owner}#{name}"))
}
