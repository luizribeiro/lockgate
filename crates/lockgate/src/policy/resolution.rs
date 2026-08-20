//! Prepare-time resolution of symbolic permission scope references.

use std::{error::Error, fmt};

use lockgate_policy::ScopeError;
use lockgate_schema::{AtomKey, GrantSet, NeedKind, NeedsManifest, ScopeRefEntry};
use serde_json::Value;

use super::CapabilityRegistry;
use crate::SymbolicRoots;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolvedNeeds {
    pub(crate) required: GrantSet,
    pub(crate) optional: GrantSet,
}

pub(crate) fn resolve_needs(
    manifest: &NeedsManifest,
    settings: &Value,
    roots: &SymbolicRoots,
    registry: &CapabilityRegistry,
) -> Result<ResolvedNeeds, ScopeResolutionError> {
    let mut resolved = ResolvedNeeds::default();
    for (entries, grants) in [
        (manifest.required(), &mut resolved.required),
        (manifest.optional(), &mut resolved.optional),
    ] {
        for entry in entries {
            let atom = entry.atom();
            let permission = registry.permission(atom).ok_or_else(|| {
                ScopeResolutionError::UnregisteredPermission { atom: atom.clone() }
            })?;
            match (entry.kind(), permission.is_scoped()) {
                (NeedKind::Flag, false) => {
                    grants.insert_flag(atom.clone());
                }
                (NeedKind::Scoped(references), true) => {
                    let scopes = references
                        .iter()
                        .map(|reference| {
                            resolve_scope(atom, reference, settings, roots, permission)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    grants
                        .insert_scopes(atom.clone(), scopes)
                        .expect("validated scoped needs contain at least one reference");
                }
                (NeedKind::Flag, true) => {
                    return Err(ScopeResolutionError::OperationKindMismatch {
                        atom: atom.clone(),
                        declared: NeedValueKind::Flag,
                        registered: NeedValueKind::Scoped,
                    });
                }
                (NeedKind::Scoped(_), false) => {
                    return Err(ScopeResolutionError::OperationKindMismatch {
                        atom: atom.clone(),
                        declared: NeedValueKind::Scoped,
                        registered: NeedValueKind::Flag,
                    });
                }
            }
        }
    }
    Ok(resolved)
}

fn resolve_scope(
    atom: &AtomKey,
    reference: &ScopeRefEntry,
    settings: &Value,
    roots: &SymbolicRoots,
    permission: lockgate_policy::__private::ErasedPermission,
) -> Result<String, ScopeResolutionError> {
    let diagnostic = ScopeReference::from(reference);
    let concrete = resolve_reference(atom, reference, settings, roots)?;
    let parsed = permission
        .parse_scope(&concrete)
        .expect("a scoped permission must retain its parse hook")
        .map_err(|source| {
            ScopeResolutionError::InvalidScope(Box::new(InvalidScopeValue {
                atom: atom.clone(),
                reference: diagnostic,
                value: concrete.clone(),
                scope_type: permission
                    .scope_type_name()
                    .expect("a scoped permission must retain its type name"),
                source,
            }))
        })?;
    Ok(permission
        .canonicalize_scope(&parsed)
        .expect("a scoped permission must retain its canonicalization hook")
        .expect("a descriptor must canonicalize values from its own parse hook"))
}

fn resolve_reference(
    atom: &AtomKey,
    reference: &ScopeRefEntry,
    settings: &Value,
    roots: &SymbolicRoots,
) -> Result<String, ScopeResolutionError> {
    match reference {
        ScopeRefEntry::Literal(value) => Ok(value.clone()),
        ScopeRefEntry::Setting(pointer) => {
            let value =
                settings
                    .pointer(pointer)
                    .ok_or_else(|| ScopeResolutionError::MissingSetting {
                        atom: atom.clone(),
                        pointer: pointer.clone(),
                    })?;
            let Value::String(value) = value else {
                return Err(ScopeResolutionError::SettingNotString {
                    atom: atom.clone(),
                    pointer: pointer.clone(),
                    found: JsonValueKind::of(value),
                });
            };
            Ok(value.clone())
        }
        ScopeRefEntry::Root { name, subpath } => {
            let root = roots
                .get(name)
                .ok_or_else(|| ScopeResolutionError::UnmappedRoot {
                    atom: atom.clone(),
                    symbol: name.clone(),
                })?;
            let root = root
                .to_str()
                .ok_or_else(|| ScopeResolutionError::NonUtf8Root {
                    atom: atom.clone(),
                    symbol: name.clone(),
                })?;
            Ok(match subpath {
                None => root.to_owned(),
                Some(subpath) if root.is_empty() || root.ends_with('/') => {
                    format!("{root}{subpath}")
                }
                Some(subpath) => format!("{root}/{subpath}"),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedValueKind {
    Flag,
    Scoped,
}

impl fmt::Display for NeedValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Flag => "unscoped flag",
            Self::Scoped => "scoped value",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonValueKind {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

impl JsonValueKind {
    fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Boolean,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::String,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
        }
    }
}

impl fmt::Display for JsonValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeReference {
    Literal,
    Setting { pointer: String },
    Root { symbol: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidScopeValue {
    pub atom: AtomKey,
    pub reference: ScopeReference,
    pub value: String,
    pub scope_type: &'static str,
    pub source: ScopeError,
}

impl From<&ScopeRefEntry> for ScopeReference {
    fn from(reference: &ScopeRefEntry) -> Self {
        match reference {
            ScopeRefEntry::Literal(_) => Self::Literal,
            ScopeRefEntry::Setting(pointer) => Self::Setting {
                pointer: pointer.clone(),
            },
            ScopeRefEntry::Root { .. } => Self::Root {
                symbol: reference.to_wire(),
            },
        }
    }
}

impl fmt::Display for ScopeReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal => formatter.write_str("literal reference"),
            Self::Setting { pointer } => write!(formatter, "setting reference `{pointer}`"),
            Self::Root { symbol } => write!(formatter, "symbolic root reference `{symbol}`"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeResolutionError {
    UnregisteredPermission {
        atom: AtomKey,
    },
    OperationKindMismatch {
        atom: AtomKey,
        declared: NeedValueKind,
        registered: NeedValueKind,
    },
    MissingSetting {
        atom: AtomKey,
        pointer: String,
    },
    SettingNotString {
        atom: AtomKey,
        pointer: String,
        found: JsonValueKind,
    },
    UnmappedRoot {
        atom: AtomKey,
        symbol: String,
    },
    NonUtf8Root {
        atom: AtomKey,
        symbol: String,
    },
    InvalidScope(Box<InvalidScopeValue>),
}

impl fmt::Display for ScopeResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnregisteredPermission { atom } => write!(
                formatter,
                "permission declaration names atom `{atom}`, but the application did not register it"
            ),
            Self::OperationKindMismatch {
                atom,
                declared,
                registered,
            } => write!(
                formatter,
                "permission atom `{atom}` is declared as an {declared}, but its registered permission is a {registered}"
            ),
            Self::MissingSetting { atom, pointer } => write!(
                formatter,
                "permission atom `{atom}` has setting reference `{pointer}`, but that JSON pointer is missing from the validated settings; expected exactly one JSON string"
            ),
            Self::SettingNotString {
                atom,
                pointer,
                found,
            } => write!(
                formatter,
                "permission atom `{atom}` has setting reference `{pointer}`, but it resolved to {found}; expected exactly one JSON string"
            ),
            Self::UnmappedRoot { atom, symbol } => write!(
                formatter,
                "permission atom `{atom}` has symbolic root reference `${symbol}`, but the host supplied no mapping for root `{symbol}`"
            ),
            Self::NonUtf8Root { atom, symbol } => write!(
                formatter,
                "permission atom `{atom}` has symbolic root reference `${symbol}`, but that host path is not valid UTF-8"
            ),
            Self::InvalidScope(error) => write!(
                formatter,
                "permission atom `{}` {} resolved to concrete value `{}`, which is not a valid `{}` scope: {}",
                error.atom, error.reference, error.value, error.scope_type, error.source,
            ),
        }
    }
}

impl Error for ScopeResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidScope(error) => Some(&error.source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use lockgate_policy::{Scope, ScopeRepr};
    use lockgate_schema::{GrantValue, NeedEntry, ScopeRefEntry};
    use serde_json::json;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TargetScope {
        All,
        Current,
        Pool(String),
        Path(String),
    }

    impl FromStr for TargetScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "all" | "everything" => Ok(Self::All),
                "current" => Ok(Self::Current),
                value
                    if value
                        .strip_prefix("pool:")
                        .is_some_and(|name| !name.is_empty()) =>
                {
                    Ok(Self::Pool(value[5..].to_owned()))
                }
                value if value.starts_with('/') => Ok(Self::Path(value.to_owned())),
                _ => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for TargetScope {
        fn canonical(&self) -> String {
            match self {
                Self::All => "all".to_owned(),
                Self::Current => "current".to_owned(),
                Self::Pool(name) => format!("pool:{name}"),
                Self::Path(path) => path.clone(),
            }
        }
    }

    impl Scope for TargetScope {}

    #[lockgate_policy::capability("sessions")]
    mod permissions {
        use super::TargetScope;
        use lockgate_policy::{Permission, ScopedPermission};

        pub const READ: ScopedPermission<TargetScope> = ScopedPermission::new("read");
        pub const SEND: Permission = Permission::new("send");
    }

    fn atom(value: &str) -> AtomKey {
        value.parse().unwrap()
    }

    fn scoped(references: Vec<ScopeRefEntry>) -> NeedEntry {
        NeedEntry::scoped(atom("sessions.read"), references).unwrap()
    }

    fn manifest(required: Vec<NeedEntry>, optional: Vec<NeedEntry>) -> NeedsManifest {
        NeedsManifest::new(required, optional).unwrap()
    }

    fn registry() -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::default();
        registry.register::<permissions::Contract>().unwrap();
        registry
    }

    #[test]
    fn resolves_every_reference_kind_and_collapses_canonical_duplicates() {
        let needs = manifest(
            vec![scoped(vec![
                ScopeRefEntry::literal("everything").unwrap(),
                ScopeRefEntry::setting("/scope").unwrap(),
                ScopeRefEntry::root("workspace")
                    .unwrap()
                    .join("shared")
                    .unwrap(),
                ScopeRefEntry::literal("all").unwrap(),
            ])],
            vec![NeedEntry::flag(atom("sessions.send"))],
        );
        let mut roots = SymbolicRoots::default();
        roots.insert("workspace", "/srv/workspace");

        let resolved =
            resolve_needs(&needs, &json!({ "scope": "current" }), &roots, &registry()).unwrap();

        assert_eq!(
            resolved.required.get(&atom("sessions.read")),
            Some(&GrantValue::Scopes(vec![
                "/srv/workspace/shared".to_owned(),
                "all".to_owned(),
                "current".to_owned(),
            ]))
        );
        assert_eq!(
            resolved.optional.get(&atom("sessions.send")),
            Some(&GrantValue::Flag)
        );
    }

    #[test]
    fn missing_and_non_string_settings_are_teaching_errors() {
        let needs = manifest(
            vec![scoped(vec![ScopeRefEntry::setting("/scope").unwrap()])],
            vec![],
        );
        let cases = [
            (json!({}), None),
            (json!({ "scope": null }), Some(JsonValueKind::Null)),
            (json!({ "scope": true }), Some(JsonValueKind::Boolean)),
            (json!({ "scope": 7 }), Some(JsonValueKind::Number)),
            (json!({ "scope": ["all"] }), Some(JsonValueKind::Array)),
            (json!({ "scope": {} }), Some(JsonValueKind::Object)),
        ];

        for (settings, found) in cases {
            let error = resolve_needs(&needs, &settings, &SymbolicRoots::default(), &registry())
                .unwrap_err();
            match (&error, found) {
                (ScopeResolutionError::MissingSetting { atom, pointer }, None) => {
                    assert_eq!(atom, &super::tests::atom("sessions.read"));
                    assert_eq!(pointer, "/scope");
                }
                (
                    ScopeResolutionError::SettingNotString {
                        atom,
                        pointer,
                        found: actual,
                    },
                    Some(expected),
                ) => {
                    assert_eq!(atom, &super::tests::atom("sessions.read"));
                    assert_eq!(pointer, "/scope");
                    assert_eq!(*actual, expected);
                }
                _ => panic!("unexpected resolution error: {error:?}"),
            }
            let message = error.to_string();
            assert!(message.contains("sessions.read"));
            assert!(message.contains("setting reference `/scope`"));
            assert!(message.contains("exactly one JSON string"));
        }
    }

    #[test]
    fn unmapped_roots_name_the_atom_and_symbol() {
        let needs = manifest(
            vec![scoped(vec![ScopeRefEntry::root("workspace").unwrap()])],
            vec![],
        );

        let error =
            resolve_needs(&needs, &json!({}), &SymbolicRoots::default(), &registry()).unwrap_err();

        assert!(matches!(
            error,
            ScopeResolutionError::UnmappedRoot { ref atom, ref symbol }
                if atom == &super::tests::atom("sessions.read") && symbol == "workspace"
        ));
        let message = error.to_string();
        assert!(message.contains("sessions.read"));
        assert!(message.contains("symbolic root reference `$workspace`"));
    }

    #[test]
    fn malformed_literals_name_reference_value_type_and_parser_error() {
        let needs = manifest(
            vec![scoped(vec![ScopeRefEntry::literal("gpu").unwrap()])],
            vec![],
        );

        let error =
            resolve_needs(&needs, &json!({}), &SymbolicRoots::default(), &registry()).unwrap_err();

        assert!(matches!(
            error,
            ScopeResolutionError::InvalidScope(ref error)
                if error.atom == super::tests::atom("sessions.read")
                    && error.reference == ScopeReference::Literal
                    && error.value == "gpu"
                    && error.scope_type.ends_with("TargetScope")
        ));
        let message = error.to_string();
        for expected in [
            "sessions.read",
            "literal reference",
            "concrete value `gpu`",
            "TargetScope",
            "unknown scope `gpu`",
        ] {
            assert!(
                message.contains(expected),
                "missing `{expected}` in {message}"
            );
        }
    }

    #[test]
    fn malformed_setting_values_retain_the_original_pointer() {
        let needs = manifest(
            vec![scoped(vec![ScopeRefEntry::setting("/pool").unwrap()])],
            vec![],
        );

        let error = resolve_needs(
            &needs,
            &json!({ "pool": "gpu" }),
            &SymbolicRoots::default(),
            &registry(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScopeResolutionError::InvalidScope(ref error)
                if error.reference
                    == ScopeReference::Setting {
                        pointer: "/pool".to_owned(),
                    }
                    && error.value == "gpu"
        ));
        assert!(error.to_string().contains("setting reference `/pool`"));
    }

    #[test]
    fn malformed_optional_references_fail_resolution_too() {
        let needs = manifest(
            vec![NeedEntry::flag(atom("sessions.send"))],
            vec![scoped(vec![ScopeRefEntry::literal("gpu").unwrap()])],
        );

        assert!(matches!(
            resolve_needs(
                &needs,
                &json!({}),
                &SymbolicRoots::default(),
                &registry(),
            ),
            Err(ScopeResolutionError::InvalidScope(ref error))
                if error.atom == super::tests::atom("sessions.read")
        ));
    }

    #[test]
    fn declarations_must_match_registered_permission_kinds() {
        let cases = [
            manifest(vec![NeedEntry::flag(atom("sessions.read"))], vec![]),
            manifest(
                vec![
                    NeedEntry::scoped(
                        atom("sessions.send"),
                        vec![ScopeRefEntry::literal("all").unwrap()],
                    )
                    .unwrap(),
                ],
                vec![],
            ),
        ];

        for needs in cases {
            let error = resolve_needs(&needs, &json!({}), &SymbolicRoots::default(), &registry())
                .unwrap_err();
            assert!(matches!(
                error,
                ScopeResolutionError::OperationKindMismatch { .. }
            ));
        }
    }
}
