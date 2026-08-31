//! Shared no-wasmtime domain types and custom-section wire formats.

/// The `config` capability WIT contract, owned here so no consumer reaches
/// across a crate boundary to include it.
pub const CONFIG_WIT: &str = include_str!("../wit/config.wit");

mod grants;
pub mod metadata;
pub mod needs;
pub mod sections;
mod text;

pub use grants::{GrantSet, GrantValue, GrantValueError};
pub use metadata::{PluginMetadata, PluginMetadataField, PluginMetadataValidationError};
pub use needs::{
    AtomKey, AtomKeyError, EntryLocation, NeedEntry, NeedEntryError, NeedKind, NeedReasonError,
    NeedsDigest, NeedsManifest, NeedsManifestValidationError, Requirement, ScopeRefEntry,
    ScopeRefEntryError, ScopeValueKind, hex_encode,
};
