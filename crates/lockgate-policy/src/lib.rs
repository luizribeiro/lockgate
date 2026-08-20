#![no_std]

//! Shared authority-contract types for Lockgate applications and plugins.

extern crate alloc;

mod atom;
mod capability;
mod env_var_name;
mod http_origin;
mod need;
mod permission;
mod scope;

pub use capability::CapabilityContract;
pub use env_var_name::EnvVarName;
pub use http_origin::HttpOrigin;
pub use need::{Need, Needs, ScopeRef};
pub use permission::{Permission, ScopedPermission};
pub use scope::{
    ExhaustiveScopeDomain, Scope, ScopeError, ScopeRepr, check_scope_laws,
    check_scope_laws_for_registration,
};

/// Derives [`ScopeRepr`] and exhaustive-domain evidence for a closed enum.
pub use lockgate_macros::{ScopeRepr, capability};

/// Built-in environment variable authority.
#[capability("env")]
pub mod env {
    use crate::{EnvVarName, ScopedPermission};

    /// Read the granted named environment variables.
    pub const READ: ScopedPermission<EnvVarName> = ScopedPermission::new("read");
}

/// Built-in outbound HTTP authority.
#[capability("net")]
pub mod net {
    use crate::{HttpOrigin, ScopedPermission};

    /// Send outbound HTTP requests to the granted exact origins.
    pub const EGRESS: ScopedPermission<HttpOrigin> = ScopedPermission::new("egress");
}

#[doc(hidden)]
pub mod __private {
    pub use crate::atom::{AtomValidationError, is_valid_authoring_id, validate_atom};
    pub use crate::capability::{
        ErasedPermission, ErasedScopeTypeError, ErasedScopeValue, erase_permission,
        erase_scoped_permission,
    };
    pub use crate::need::{
        need_capability, need_permission, need_scopes, needs_format, needs_optional,
        needs_required, scope_ref_wire_byte, scope_ref_wire_len,
    };
    pub use crate::permission::{
        PermissionDecl, ScopedPermissionDecl, permission_ids, qualify_permission,
        qualify_scoped_permission, scoped_permission_ids,
    };
    pub use alloc::{string::String, vec};
}
