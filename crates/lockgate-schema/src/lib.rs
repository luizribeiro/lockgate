//! Shared no-wasmtime domain types and custom-section wire formats.

mod grants;
pub mod metadata;
pub mod needs;
pub mod sections;
mod text;

pub use grants::{GrantSet, GrantValue, GrantValueError};
pub use metadata::{PluginMetadata, PluginMetadataField, PluginMetadataValidationError};
pub use needs::{
    AtomKey, AtomKeyError, EntryLocation, NeedEntry, NeedEntryError, NeedKind, NeedReasonError,
    NeedsDigest, NeedsManifest, NeedsManifestValidationError, Requirement, ScopeRef, ScopeRefError,
    ScopeValueKind, hex_encode,
};
