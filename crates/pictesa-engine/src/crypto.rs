//! Encrypts stored provider secrets with a per-instance key (XChaCha20-Poly1305).
//! The key lives next to the database with owner-only permissions; losing it only means
//! re-entering API keys.

use std::path::Path;

use anyhow::{anyhow, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};

pub struct Crypto {
    cipher: XChaCha20Poly1305,
    mac_key: [u8; 32],
}

impl Crypto {
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        let key = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match create_key(path) {
                Ok(key) => key,
                // Another process (a CLI next to a starting server) created it first: use theirs.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::fs::read(path).context("reading instance key")?,
                Err(e) => return Err(e).context("writing instance key"),
            },
            Err(e) => return Err(e).context("reading instance key"),
        };
        if key.len() != 32 {
            return Err(anyhow!("{} is not a valid 32-byte key", path.display()));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow!("bad key length"))?;
        let mac_key = blake3::derive_key("pictesa 2026 signed file urls", &key);
        Ok(Self { cipher, mac_key })
    }

    /// Short keyed signature, used for unguessable file URLs handed to providers.
    pub fn sign(&self, data: &str) -> String {
        blake3::keyed_hash(&self.mac_key, data.as_bytes()).to_hex()[..32].to_string()
    }

    pub fn encrypt(&self, plain: &str) -> anyhow::Result<String> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = self.cipher.encrypt(&nonce, plain.as_bytes()).map_err(|_| anyhow!("encryption failed"))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        Ok(STANDARD.encode(out))
    }

    pub fn decrypt(&self, sealed: &str) -> anyhow::Result<String> {
        let raw = STANDARD.decode(sealed).context("stored secret is not base64")?;
        if raw.len() < 24 {
            return Err(anyhow!("stored secret is too short"));
        }
        let (nonce, data) = raw.split_at(24);
        let plain = self.cipher.decrypt(XNonce::from_slice(nonce), data).map_err(|_| anyhow!("stored secret cannot be decrypted with this instance key"))?;
        Ok(String::from_utf8(plain)?)
    }
}

/// Writes a new key so that it appears complete or not at all: a private temporary file, then a
/// hard link that fails with `AlreadyExists` when another process got there first.
fn create_key(path: &Path) -> std::io::Result<Vec<u8>> {
    let key = XChaCha20Poly1305::generate_key(&mut OsRng).to_vec();
    let tmp = path.with_extension(format!("tmp-{}", crate::new_id()));
    write_private(&tmp, &key)?;
    let linked = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    linked.map(|()| key)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
        f.sync_all()
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_starting_together_share_one_key() {
        let dir = std::env::temp_dir().join(format!("pictesa-key-{}", crate::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("instance.key");
        let signatures: Vec<String> = std::thread::scope(|s| {
            let runs: Vec<_> = (0..8).map(|_| s.spawn(|| Crypto::load_or_create(&path).unwrap().sign("x"))).collect();
            runs.into_iter().map(|r| r.join().unwrap()).collect()
        });
        assert!(signatures.windows(2).all(|w| w[0] == w[1]), "all starts use the same key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
