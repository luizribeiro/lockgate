//! Exact environment variable name scopes.

use alloc::string::String;
use core::{fmt, str::FromStr};

use crate::{Scope, ScopeError, ScopeRepr};

/// One exact, case-sensitive environment variable name authorized for reading.
///
/// Names are opaque: prefixes, globs, and other patterns have no special
/// meaning.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvVarName(String);

impl fmt::Debug for EnvVarName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("EnvVarName")
            .field(&self.canonical())
            .finish()
    }
}

impl FromStr for EnvVarName {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.as_bytes().contains(&b'=') || value.as_bytes().contains(&0) {
            return Err(ScopeError::unknown(value));
        }
        Ok(Self(value.into()))
    }
}

impl ScopeRepr for EnvVarName {
    fn canonical(&self) -> String {
        self.0.clone()
    }
}

impl Scope for EnvVarName {}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use super::EnvVarName;
    use crate::{Scope, ScopeRepr, check_scope_laws};

    #[test]
    fn env_var_name_samples_obey_scope_laws() {
        check_scope_laws([
            EnvVarName::from_str("PATH").unwrap(),
            EnvVarName::from_str("Path").unwrap(),
            EnvVarName::from_str("PATH_PREFIX").unwrap(),
            EnvVarName::from_str("*").unwrap(),
        ])
        .unwrap();
    }

    #[test]
    fn env_var_name_parsing_preserves_exact_names() {
        for accepted in ["PATH", "Path", "_TOKEN", "service.token", "*", " name "] {
            let name = EnvVarName::from_str(accepted).unwrap();
            assert_eq!(name.canonical(), accepted);
            assert_eq!(EnvVarName::from_str(&name.canonical()).unwrap(), name);
        }

        let path = EnvVarName::from_str("PATH").unwrap();
        assert!(path.contains(&EnvVarName::from_str("PATH").unwrap()));
        assert!(!path.contains(&EnvVarName::from_str("Path").unwrap()));
        assert!(!path.contains(&EnvVarName::from_str("PATH_PREFIX").unwrap()));
        assert!(!EnvVarName::from_str("*").unwrap().contains(&path));
    }

    #[test]
    fn env_var_name_parsing_rejects_empty_equals_and_nul() {
        for invalid in ["", "NAME=value", "\0", "NAME\0SUFFIX"] {
            assert!(
                EnvVarName::from_str(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
