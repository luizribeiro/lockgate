//! Static validation for the value-only export boundary.
//!
//! Every exported interface must avoid resources, whether or not a resource
//! declaration is used, as well as `own<T>`, `borrow<T>`, `future`, `stream`,
//! and `error-context`. The same restrictions apply when those shapes are
//! hidden inside nested type aliases. World-level exported functions and
//! types are also rejected.
//!
//! The concern is whether a value can escape an invocation, rather than the
//! types in isolation. Each invocation receives a fresh Wasmtime Store, which
//! is dropped when the call returns. A handle, stream, or future that outlived
//! that call would either dangle or require the Store to remain alive.
//! Value-in/value-out signatures therefore make drop-on-cancel and a separate
//! instance per invocation safe.
//!
//! Execution remains asynchronous: the host calls every export asynchronously,
//! and a guest can await asynchronous host imports during an invocation.
//! Validation uses `wit-parser` before Wasmtime compilation so failures produce
//! a teaching error instead of a generic feature error. It inspects every WIT
//! export, including interfaces the embedding application has not registered,
//! because an unknown interface can become wired in the future.
//!
//! Forbidden shapes are matched exhaustively, without a catch-all arm. New WIT
//! type kinds must therefore be considered here before the crate will build.

use std::{error::Error, fmt};

use wit_parser::{
    Handle, InterfaceId, Resolve, Type, TypeDefKind, TypeId, WorldId, WorldItem, WorldKey,
    decoding::{DecodedWasm, decode},
};

/// Validates the value-only rule directly against a component's decoded WIT.
pub(crate) fn validate_value_only_exports(bytes: &[u8]) -> Result<(), ValidationError> {
    let decoded = decode(bytes).map_err(classify_decode_error)?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(ValidationError::NotComponent);
    };
    validate_world(&resolve, world)
}

fn classify_decode_error(error: anyhow::Error) -> ValidationError {
    let message = error.to_string();
    // This is private wording from wit-parser's `decode_component_export`.
    // A dependency wording change will fail the world-export golden test; the
    // same branch covers rare module/value exports, so all are deliberately
    // reported under the honest world-level non-interface export label.
    if let Some(name) = message
        .strip_prefix("component export `")
        .and_then(|message| message.strip_suffix("` was not a function or instance"))
    {
        return ValidationError::unsupported(
            "root",
            format!("<type {name}>"),
            "world-level non-interface export",
        );
    }
    ValidationError::Decode { message }
}

fn validate_world(resolve: &Resolve, world: WorldId) -> Result<(), ValidationError> {
    let world = &resolve.worlds[world];
    for (key, item) in &world.exports {
        let export_name = match key {
            WorldKey::Name(name) => name.clone(),
            WorldKey::Interface(id) => resolve
                .id_of(*id)
                .unwrap_or_else(|| format!("interface-{}", id.index())),
        };
        match item {
            WorldItem::Interface { id, .. } => validate_interface(resolve, *id, &export_name)?,
            WorldItem::Function(_) => {
                return Err(ValidationError::unsupported(
                    &world.name,
                    export_name,
                    "world-level exported function",
                ));
            }
            WorldItem::Type { .. } => {
                // Kept as forward-compat insurance; decoded component exports do not reach this.
                return Err(ValidationError::unsupported(
                    &world.name,
                    format!("<type {export_name}>"),
                    "world-level exported type",
                ));
            }
        }
    }
    Ok(())
}

fn validate_interface(
    resolve: &Resolve,
    interface: InterfaceId,
    interface_name: &str,
) -> Result<(), ValidationError> {
    for function in resolve.interfaces[interface].functions.values() {
        let types = function
            .params
            .iter()
            .map(|param| param.ty)
            .chain(function.result);
        for ty in types {
            if let Some(offending_type) = forbidden_type(resolve, ty) {
                return Err(ValidationError::unsupported(
                    interface_name,
                    &function.name,
                    offending_type,
                ));
            }
        }
    }
    for (name, id) in &resolve.interfaces[interface].types {
        let resolved = resolve_alias(resolve, *id);
        if matches!(resolve.types[resolved].kind, TypeDefKind::Resource) {
            return Err(ValidationError::unsupported(
                interface_name,
                format!("<type {name}>"),
                format!("resource {}", type_name(resolve, resolved)),
            ));
        }
    }
    Ok(())
}

fn forbidden_type(resolve: &Resolve, ty: Type) -> Option<String> {
    let id = match ty {
        Type::ErrorContext => return Some("error-context".to_string()),
        Type::Id(id) => resolve_alias(resolve, id),
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::S8
        | Type::S16
        | Type::S32
        | Type::S64
        | Type::F32
        | Type::F64
        | Type::Char
        | Type::String => return None,
    };
    let definition = &resolve.types[id];
    match &definition.kind {
        TypeDefKind::Resource => Some(format!("resource {}", type_name(resolve, id))),
        TypeDefKind::Handle(handle) => Some(match handle {
            Handle::Own(resource) => format!("own<{}>", type_name(resolve, *resource)),
            Handle::Borrow(resource) => format!("borrow<{}>", type_name(resolve, *resource)),
        }),
        TypeDefKind::Record(record) => record
            .fields
            .iter()
            .find_map(|field| forbidden_type(resolve, field.ty)),
        TypeDefKind::Tuple(tuple) => tuple
            .types
            .iter()
            .find_map(|ty| forbidden_type(resolve, *ty)),
        TypeDefKind::Variant(variant) => variant
            .cases
            .iter()
            .filter_map(|case| case.ty)
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Option(ty)
        | TypeDefKind::List(ty)
        | TypeDefKind::FixedLengthList(ty, _)
        | TypeDefKind::Type(ty) => forbidden_type(resolve, *ty),
        TypeDefKind::Result(result) => result
            .ok
            .into_iter()
            .chain(result.err)
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Map(key, value) => [*key, *value]
            .into_iter()
            .find_map(|ty| forbidden_type(resolve, ty)),
        TypeDefKind::Future(_) => Some("future".to_string()),
        TypeDefKind::Stream(_) => Some("stream".to_string()),
        TypeDefKind::Flags(_) | TypeDefKind::Enum(_) | TypeDefKind::Unknown => None,
    }
}

fn resolve_alias(resolve: &Resolve, id: TypeId) -> TypeId {
    match resolve.types[id].kind {
        TypeDefKind::Type(Type::Id(target)) => resolve_alias(resolve, target),
        _ => id,
    }
}

fn type_name(resolve: &Resolve, id: TypeId) -> String {
    resolve.types[id]
        .name
        .clone()
        .unwrap_or_else(|| format!("type-{}", id.index()))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ValidationError {
    Decode {
        message: String,
    },
    NotComponent,
    UnsupportedExport {
        interface: String,
        function: String,
        offending_type: String,
    },
}

impl ValidationError {
    /// Returns the stable machine-readable code assigned to this error.
    ///
    /// Codes are append-only: once assigned, a code is never changed or reused.
    pub(crate) fn code(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedExport { .. } => Some("admission.unsupported-export"),
            Self::Decode { .. } | Self::NotComponent => None,
        }
    }

    fn unsupported(
        interface: impl Into<String>,
        function: impl Into<String>,
        offending_type: impl Into<String>,
    ) -> Self {
        Self::UnsupportedExport {
            interface: interface.into(),
            function: function.into(),
            offending_type: offending_type.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { message } => {
                write!(formatter, "failed to decode component WIT: {message}")
            }
            Self::NotComponent => formatter.write_str("input is a WIT package, not a component"),
            Self::UnsupportedExport {
                interface,
                function,
                offending_type,
            } => {
                write!(
                    formatter,
                    "[admission.unsupported-export] unsupported export `{interface}#{function}`: offending type `{offending_type}` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
                )?;
                if matches!(offending_type.as_str(), "future" | "stream") {
                    formatter.write_str(
                        ". Async signatures are often generated by guest toolchains by default; the exported WIT function must be a synchronous value-returning function, but the host call may still be asynchronous.",
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl Error for ValidationError {}

#[cfg(test)]
mod tests;
