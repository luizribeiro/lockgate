#![no_std]

//! Shared authority-contract types for Lockgate applications and plugins.

extern crate alloc;

mod scope;

pub use scope::{
    ExhaustiveScopeDomain, Scope, ScopeError, ScopeRepr, check_scope_laws,
    check_scope_laws_for_registration,
};
