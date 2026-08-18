#![no_std]

//! Shared authority-contract types for Lockgate applications and plugins.

extern crate alloc;

mod atom;
mod capability;
mod permission;
mod scope;

pub use capability::CapabilityContract;
pub use permission::{Permission, ScopedPermission};
pub use scope::{
    ExhaustiveScopeDomain, Scope, ScopeError, ScopeRepr, check_scope_laws,
    check_scope_laws_for_registration,
};

/// Derives [`ScopeRepr`] and exhaustive-domain evidence for a closed enum.
pub use lockgate_macros::ScopeRepr;

#[doc(hidden)]
pub mod __private {
    pub use crate::atom::{AtomValidationError, is_valid_authoring_id, validate_atom};
    pub use crate::capability::{
        ErasedPermission, ErasedScopeTypeError, ErasedScopeValue, erase_permission,
        erase_scoped_permission,
    };
    pub use crate::permission::{
        PermissionDecl, ScopedPermissionDecl, qualify_permission, qualify_scoped_permission,
    };
    pub use alloc::{string::String, vec};
}
