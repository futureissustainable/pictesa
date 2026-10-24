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
            Ok(bytes) if bytes.len() == 32 => bytes,
            Ok(_) => return Err(anyhow!("{} is not a valid 32-byte key", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = XChaCha20Poly1305::generate_key(&mut OsRng);
                write_private(path, key.as_slice())?;
                key.to_vec()
            }
            Err(e) => return Err(e).context("reading instance key"),
        };
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

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}
