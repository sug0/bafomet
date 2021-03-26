use ring::digest::{
    self,
    SHA256,
    SHA256_OUTPUT_LEN,
};

use crate::bft::error::*;

pub struct Context;

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct Digest([u8; Digest::LENGTH]);

impl Digest {
    pub const LENGTH: usize = SHA256_OUTPUT_LEN;

    pub fn from_bytes(raw_bytes: &[u8]) -> Result<Self> {
        if raw_bytes.len() < Self::LENGTH {
            return Err("Digest has an invalid length")
                .wrapped(ErrorKind::CryptoHashRingSha2);
        }
        Ok(Self::from_bytes_unchecked(raw_bytes))
    }

    fn from_bytes_unchecked(raw_bytes: &[u8]) -> Self {
        let mut inner = [0; Self::LENGTH];
        inner.copy_from_slice(&raw_bytes[..Self::LENGTH]);
        Self(inner)
    }
}

impl AsRef<[u8]> for Digest {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
