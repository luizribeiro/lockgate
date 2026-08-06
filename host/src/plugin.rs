//! Component discovery and structural WIT metadata.
//! Decodes binaries, validates manifests against their worlds, and records callable targets.

use crate::manifest::Manifest;
use anyhow::{Context, Result, bail};
use std::{collections::HashMap, fmt, fs, path::Path};
use wasmtime::{Engine, component::Component};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, Type, TypeDefKind, WorldItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) params: Vec<String>,
    pub(crate) result: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) plugin: String,
    pub(crate) interface: String,
    pub(crate) function: String,
    pub(crate) signature: Signature,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectImport {
    pub(crate) interface: String,
    pub(crate) functions: Vec<(String, Signature)>,
}

pub(crate) struct PluginDefinition {
    pub(crate) manifest: Manifest,
    pub(crate) component: Component,
    pub(crate) targets: Vec<(String, Target)>,
    pub(crate) direct_imports: Vec<DirectImport>,
}

impl fmt::Display for Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "({}) -> {}",
            self.params.join(", "),
            self.result.as_deref().unwrap_or("()")
        )
    }
}

impl Target {
    pub(crate) fn key(&self) -> String {
        format!("{}#{}", self.interface, self.function)
    }
}

impl PluginDefinition {
    pub(crate) fn load(engine: &Engine, root: &Path, id: &str) -> Result<Self> {
        let dir = root.join("plugins").join(id);
        let manifest: Manifest = toml::from_str(&fs::read_to_string(dir.join("plugin.toml"))?)?;
        if manifest.id != id {
            bail!("manifest id {:?} does not match directory", manifest.id);
        }
        let bytes = fs::read(dir.join(format!("{id}.wasm"))).context("component binary missing")?;
        let DecodedWasm::Component(resolve, world) = wit_component::decode(&bytes)? else {
            bail!("binary is not a component");
        };
        let direct_imports = decode_imports(&resolve, world, &manifest)?;
        let exports = exported_interfaces(&resolve, world)?;
        for provided in &manifest.provides {
            if !exports.contains_key(provided) {
                bail!("manifest provides {provided}, but component does not export it");
            }
        }
        let mut targets = Vec::new();
        for provided in &manifest.provides {
            for function in resolve.interfaces[exports[provided]].functions.values() {
                let target = Target {
                    plugin: id.into(),
                    interface: provided.clone(),
                    function: function.name.clone(),
                    signature: signature(&resolve, function),
                };
                targets.push((target.key(), target));
            }
        }
        Ok(Self {
            manifest,
            component: Component::new(engine, bytes)?,
            targets,
            direct_imports,
        })
    }
}

pub(crate) fn decode_imports(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    manifest: &Manifest,
) -> Result<Vec<DirectImport>> {
    let mut direct = Vec::new();
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            bail!("direct world functions are unsupported");
        };
        let name = resolve.id_of(*id).context("unnamed imported interface")?;
        if name == "tangent:core/registry@0.1.0" {
            if !manifest.capabilities.registry {
                bail!("undeclared import {name}");
            }
        } else if name.starts_with("wasi:filesystem/") {
            if manifest.capabilities.fs.is_none() {
                bail!("undeclared import {name}");
            }
        } else if name.starts_with("wasi:sockets/") {
            if manifest
                .capabilities
                .net
                .as_ref()
                .is_none_or(|net| net.hosts.is_empty())
            {
                bail!("undeclared import {name}");
            }
        } else if !name.starts_with("wasi:") {
            let functions = resolve.interfaces[*id]
                .functions
                .values()
                .map(|function| (function.name.clone(), signature(resolve, function)))
                .collect();
            direct.push(DirectImport {
                interface: name,
                functions,
            });
        }
    }
    Ok(direct)
}

fn exported_interfaces(
    resolve: &Resolve,
    world: wit_parser::WorldId,
) -> Result<HashMap<String, wit_parser::InterfaceId>> {
    let mut exports = HashMap::new();
    for item in resolve.worlds[world].exports.values() {
        if let WorldItem::Interface { id, .. } = item {
            exports.insert(
                resolve.id_of(*id).context("unnamed exported interface")?,
                *id,
            );
        }
    }
    Ok(exports)
}

fn signature(resolve: &Resolve, function: &wit_parser::Function) -> Signature {
    Signature {
        params: function
            .params
            .iter()
            .map(|param| type_shape(resolve, param.ty))
            .collect(),
        result: function.result.map(|ty| type_shape(resolve, ty)),
    }
}

fn type_shape(resolve: &Resolve, ty: Type) -> String {
    match ty {
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
        Type::Id(id) => match &resolve.types[id].kind {
            TypeDefKind::Type(ty) => type_shape(resolve, *ty),
            TypeDefKind::List(ty) => format!("list<{}>", type_shape(resolve, *ty)),
            TypeDefKind::Option(ty) => format!("option<{}>", type_shape(resolve, *ty)),
            TypeDefKind::Tuple(tuple) => format!(
                "tuple<{}>",
                tuple
                    .types
                    .iter()
                    .map(|ty| type_shape(resolve, *ty))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            TypeDefKind::Record(record) => format!(
                "record{{{}}}",
                record
                    .fields
                    .iter()
                    .map(|field| format!("{}:{}", field.name, type_shape(resolve, field.ty)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            TypeDefKind::Result(result) => format!(
                "result<{},{}>",
                result
                    .ok
                    .map(|ty| type_shape(resolve, ty))
                    .unwrap_or_default(),
                result
                    .err
                    .map(|ty| type_shape(resolve, ty))
                    .unwrap_or_default()
            ),
            kind => kind.as_str().into(),
        },
    }
}

pub(crate) fn print_load(definition: &PluginDefinition) {
    let id = &definition.manifest.id;
    let targets = definition
        .targets
        .iter()
        .map(|(_, target)| format!("{}{}", target.key(), target.signature))
        .collect::<Vec<_>>()
        .join(", ");
    println!("[load] {id:<12} ok   provides {targets}");
}
