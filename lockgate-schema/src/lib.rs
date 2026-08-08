use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
#[cfg(feature = "wit")]
use wit_parser::{
    Function, FunctionKind, Handle, Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldId,
    WorldItem,
};

/// Name of the WebAssembly custom section containing a Lockgate plugin manifest.
pub const PLUGIN_METADATA_SECTION: &str = "lockgate:plugin";

const PLUGIN_METADATA_FORMAT: u32 = 1;

/// Language-neutral identity and display metadata embedded in a plugin artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMetadata {
    format: u32,
    id: String,
    name: String,
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
}

impl PluginMetadata {
    /// Creates and validates required plugin metadata.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self> {
        let metadata = Self {
            format: PLUGIN_METADATA_FORMAT,
            id: id.into(),
            name: name.into(),
            version: version.into(),
            description: None,
            license: None,
            repository: None,
            homepage: None,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    /// Adds a human-readable description.
    pub fn with_description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Adds a license identifier or expression.
    pub fn with_license(mut self, value: impl Into<String>) -> Self {
        self.license = Some(value.into());
        self
    }

    /// Adds the plugin's source repository URL.
    pub fn with_repository(mut self, value: impl Into<String>) -> Self {
        self.repository = Some(value.into());
        self
    }

    /// Adds the plugin's homepage URL.
    pub fn with_homepage(mut self, value: impl Into<String>) -> Self {
        self.homepage = Some(value.into());
        self
    }

    /// Returns the stable machine-readable plugin identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the human-readable plugin name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the plugin implementation's SemVer version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the optional human-readable description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the optional license identifier or expression.
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Returns the optional source repository URL.
    pub fn repository(&self) -> Option<&str> {
        self.repository.as_deref()
    }

    /// Returns the optional homepage URL.
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    fn validate(&self) -> Result<()> {
        if self.format != PLUGIN_METADATA_FORMAT {
            bail!("unsupported plugin metadata format {}", self.format);
        }
        validate_required("plugin id", &self.id)?;
        if self.id.chars().any(char::is_whitespace) {
            bail!("plugin id must not contain whitespace");
        }
        validate_required("plugin name", &self.name)?;
        validate_required("plugin version", &self.version)?;
        semver::Version::parse(&self.version)
            .map_err(|error| anyhow::anyhow!("plugin version is not valid SemVer: {error}"))?;
        for (field, value) in [
            ("description", self.description.as_deref()),
            ("license", self.license.as_deref()),
            ("repository", self.repository.as_deref()),
            ("homepage", self.homepage.as_deref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                bail!("plugin {field} must not be empty when present");
            }
        }
        Ok(())
    }
}

/// Serializes validated metadata for the `lockgate:plugin` custom section.
pub fn encode_plugin_metadata(metadata: &PluginMetadata) -> Result<Vec<u8>> {
    metadata.validate()?;
    serde_json::to_vec(metadata).map_err(Into::into)
}

/// Decodes and validates metadata from the `lockgate:plugin` custom section.
pub fn decode_plugin_metadata(bytes: &[u8]) -> Result<PluginMetadata> {
    let metadata: PluginMetadata = serde_json::from_slice(bytes)?;
    metadata.validate()?;
    Ok(metadata)
}

fn validate_required(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    if value.trim() != value {
        bail!("{field} must not have leading or trailing whitespace");
    }
    Ok(())
}

#[cfg(feature = "wit")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub interface: String,
    pub item: String,
    pub signature: String,
}

#[cfg(feature = "wit")]
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

#[cfg(feature = "wit")]
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

#[cfg(feature = "wit")]
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

#[cfg(feature = "wit")]
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

#[cfg(feature = "wit")]
fn optional_type_name(resolve: &Resolve, ty: Option<Type>) -> Result<String> {
    ty.map(|ty| type_name(resolve, ty))
        .transpose()
        .map(|ty| ty.unwrap_or_else(|| "unit".into()))
}

#[cfg(feature = "wit")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_metadata_has_a_stable_language_neutral_encoding() {
        let metadata = PluginMetadata::new("com.example.greeter", "Greeter", "1.2.3")
            .unwrap()
            .with_description("Returns greetings");
        let encoded = encode_plugin_metadata(&metadata).unwrap();

        assert_eq!(
            std::str::from_utf8(&encoded).unwrap(),
            r#"{"format":1,"id":"com.example.greeter","name":"Greeter","version":"1.2.3","description":"Returns greetings"}"#
        );
        assert_eq!(decode_plugin_metadata(&encoded).unwrap(), metadata);
    }

    #[test]
    fn plugin_metadata_requires_semver() {
        let error = PluginMetadata::new("com.example.greeter", "Greeter", "latest").unwrap_err();

        assert!(error.to_string().contains("valid SemVer"));
    }
}
