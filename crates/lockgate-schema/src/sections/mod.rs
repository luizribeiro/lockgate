//! Custom-section names and wire-format encode/decode boundaries.

pub mod metadata;
pub mod needs;

/// Name of the WebAssembly custom section containing Lockgate plugin metadata.
pub const PLUGIN_METADATA_SECTION: &str = "lockgate:plugin";

/// Name of the WebAssembly custom section containing symbolic permission needs.
pub const PLUGIN_NEEDS_SECTION: &str = "lockgate:needs";

/// Maximum JSON payload size for either Lockgate custom section.
pub const MAX_SECTION_PAYLOAD_BYTES: usize = 1_048_576;

#[derive(Clone, Copy)]
pub(crate) struct PayloadTooLarge {
    pub(crate) actual_bytes: usize,
    pub(crate) max_bytes: usize,
}

pub(crate) fn check_payload_size(actual_bytes: usize) -> Result<(), PayloadTooLarge> {
    if actual_bytes > MAX_SECTION_PAYLOAD_BYTES {
        return Err(PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        });
    }
    Ok(())
}
