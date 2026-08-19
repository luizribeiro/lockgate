use std::fmt;

use sha2::{Digest, Sha256};

use crate::sections::needs::EncodeError;

use super::NeedsManifest;

/// Encodes a 32-byte digest as lowercase hexadecimal.
pub fn hex_encode(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// SHA-256 of a canonical symbolic needs manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NeedsDigest([u8; 32]);

impl NeedsDigest {
    /// Computes the digest over the manifest's canonical wire encoding.
    pub fn compute(manifest: &NeedsManifest) -> Result<Self, EncodeError> {
        let bytes = manifest.to_section_bytes()?;
        Ok(Self(Sha256::digest(bytes).into()))
    }

    /// Returns the raw SHA-256 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the lowercase 64-character hexadecimal digest.
    pub fn to_hex(self) -> String {
        hex_encode(&self.0)
    }
}

impl fmt::Display for NeedsDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:{}", self.to_hex())
    }
}
