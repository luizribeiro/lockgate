//! Stable permission atom identity.
//!
//! Atom segments are explicit wire names rather than Rust identifiers. Keeping
//! both segments separately prevents an unqualified permission declaration
//! from being mistaken for a complete authority identity.

use core::fmt;

/// A validated `<capability-id>.<permission-id>` identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct QualifiedAtom {
    capability: &'static str,
    permission: &'static str,
}

impl QualifiedAtom {
    /// Validates and combines the two explicit stable identity segments.
    pub(crate) const fn new(
        capability: &'static str,
        permission: &'static str,
    ) -> Result<Self, AtomValidationError> {
        match validate_atom(capability, permission) {
            Ok(()) => Ok(Self {
                capability,
                permission,
            }),
            Err(error) => Err(error),
        }
    }

    pub(crate) const fn capability(self) -> &'static str {
        self.capability
    }

    pub(crate) const fn permission(self) -> &'static str {
        self.permission
    }
}

/// A malformed stable permission atom.
///
/// This hidden type is exposed for macro-generated const validation. Its
/// variants deliberately mirror the frozen `lockgate-schema` atom grammar.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomValidationError {
    /// The capability ID is empty.
    EmptyCapability,
    /// The permission ID is empty.
    EmptyPermission,
    /// One segment contains `.`, which would make the atom ambiguous.
    DotInSegment,
}

impl fmt::Display for AtomValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCapability => {
                formatter.write_str("permission atom capability ID must not be empty")
            }
            Self::EmptyPermission => {
                formatter.write_str("permission atom permission ID must not be empty")
            }
            Self::DotInSegment => formatter.write_str(
                "permission atom must have exactly two segments: capability.permission; neither ID may contain `.`",
            ),
        }
    }
}

/// Validates stable capability and permission IDs in const contexts.
///
/// The accepted grammar is the same as `lockgate_schema::AtomKey`: both IDs
/// are non-empty and neither contains `.`. In particular, the frozen wire
/// format places no additional character or byte-length restriction on these
/// segments.
#[doc(hidden)]
pub const fn validate_atom(capability: &str, permission: &str) -> Result<(), AtomValidationError> {
    if capability.is_empty() {
        return Err(AtomValidationError::EmptyCapability);
    }
    if permission.is_empty() {
        return Err(AtomValidationError::EmptyPermission);
    }
    if contains_dot(capability) || contains_dot(permission) {
        return Err(AtomValidationError::DotInSegment);
    }
    Ok(())
}

/// Validates the stricter grammar used for author-written stable IDs.
///
/// This is deliberately separate from [`validate_atom`], whose permissive
/// grammar is frozen for decoding existing wire data. Capability macros and
/// host registration use this check at their respective trust boundaries.
#[doc(hidden)]
pub const fn is_valid_authoring_id(value: &str) -> bool {
    // WHY: This must remain const-usable, while the proc-macro crate cannot depend back on
    // `lockgate-policy` without a cycle. Keep this algorithm and its boundary vectors in sync
    // with `lockgate-macros/src/capability.rs::validate_id`.
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let mut index = 0;
    let mut previous_was_dash = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'-' {
            if previous_was_dash {
                return false;
            }
            previous_was_dash = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_was_dash = false;
        } else {
            return false;
        }
        index += 1;
    }
    !previous_was_dash
}

pub(crate) const fn validate_capability_id(capability: &str) -> Result<(), AtomValidationError> {
    if capability.is_empty() {
        Err(AtomValidationError::EmptyCapability)
    } else if contains_dot(capability) {
        Err(AtomValidationError::DotInSegment)
    } else {
        Ok(())
    }
}

pub(crate) const fn validate_permission_id(permission: &str) -> Result<(), AtomValidationError> {
    if permission.is_empty() {
        Err(AtomValidationError::EmptyPermission)
    } else if contains_dot(permission) {
        Err(AtomValidationError::DotInSegment)
    } else {
        Ok(())
    }
}

const fn contains_dot(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'.' {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use lockgate_schema::{AtomKey, AtomKeyError};

    #[derive(Debug, PartialEq, Eq)]
    enum ValidationOutcome {
        Valid,
        EmptyCapability,
        EmptyPermission,
        DotInSegment,
    }

    const CONST_VALIDATION: Result<(), AtomValidationError> = validate_atom("vm", "exec");

    #[test]
    fn validation_is_const_usable() {
        assert_eq!(CONST_VALIDATION, Ok(()));
    }

    #[test]
    fn qualified_identity_retains_both_explicit_segments() {
        let atom = QualifiedAtom::new("vm", "exec").unwrap();

        assert_eq!(atom.capability(), "vm");
        assert_eq!(atom.permission(), "exec");
    }

    #[test]
    fn validation_matches_the_frozen_schema_grammar() {
        let vectors = [
            ("vm", "exec"),
            ("virtual-machines", "edit-history"),
            ("é", "读取"),
            (" capability ", " permission "),
            ("line\nbreak", "tab\tname"),
            ("", "read"),
            ("sessions", ""),
            ("", ""),
            ("sessions.admin", ""),
            ("", "history.read"),
            ("sessions.admin", "read"),
            ("sessions", "history.read"),
            (".", "."),
        ];

        for (capability, permission) in vectors {
            assert_eq!(
                policy_outcome(validate_atom(capability, permission)),
                schema_outcome(AtomKey::new(capability, permission)),
                "policy and schema validation differ for {capability:?}.{permission:?}",
            );
        }
    }

    #[test]
    fn errors_explain_how_to_correct_each_invalid_shape() {
        assert_eq!(
            validate_atom("", "read").unwrap_err().to_string(),
            "permission atom capability ID must not be empty",
        );
        assert_eq!(
            validate_atom("sessions", "").unwrap_err().to_string(),
            "permission atom permission ID must not be empty",
        );
        assert_eq!(
            validate_atom("sessions.admin", "read")
                .unwrap_err()
                .to_string(),
            "permission atom must have exactly two segments: capability.permission; neither ID may contain `.`",
        );
    }

    #[test]
    fn authoring_id_boundary_vectors_match_macro() {
        // Keep this literal table verbatim with capability.rs::authoring_id_boundary_vectors_match_policy.
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
            assert_eq!(
                is_valid_authoring_id(id),
                expected_valid,
                "unexpected authoring-ID result for {id:?}",
            );
        }
    }

    fn policy_outcome(result: Result<(), AtomValidationError>) -> ValidationOutcome {
        match result {
            Ok(()) => ValidationOutcome::Valid,
            Err(AtomValidationError::EmptyCapability) => ValidationOutcome::EmptyCapability,
            Err(AtomValidationError::EmptyPermission) => ValidationOutcome::EmptyPermission,
            Err(AtomValidationError::DotInSegment) => ValidationOutcome::DotInSegment,
        }
    }

    fn schema_outcome(result: Result<AtomKey, AtomKeyError>) -> ValidationOutcome {
        match result {
            Ok(_) => ValidationOutcome::Valid,
            Err(AtomKeyError::EmptyCapability) => ValidationOutcome::EmptyCapability,
            Err(AtomKeyError::EmptyOperation) => ValidationOutcome::EmptyPermission,
            Err(AtomKeyError::WrongSegmentCount { .. }) => ValidationOutcome::DotInSegment,
            Err(error) => panic!("unexpected schema atom error: {error}"),
        }
    }
}
