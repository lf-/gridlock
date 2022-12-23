//! Implementation of hashing nar files.

use std::io::Write;

use sha2::Digest;

pub struct NarHasher(sha2::Sha256);

/// Subresource integrity hash
type SRIHash = String;

impl NarHasher {
    pub fn new() -> NarHasher {
        NarHasher(sha2::Sha256::default())
    }

    pub fn digest(self) -> SRIHash {
        let digest = self.0.finalize();
        let base64d = base64::encode(digest);
        format!("sha256-{}", base64d)
    }
}

impl Write for NarHasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
