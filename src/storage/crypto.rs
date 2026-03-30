use anyhow::{Context, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    AeadCore, ChaCha20Poly1305, Key, Nonce,
};

/// Nonce size for ChaCha20Poly1305 (96 bits / 12 bytes).
const NONCE_LEN: usize = 12;

/// Encrypt `plaintext` with ChaCha20Poly1305 and the provided key.
///
/// Returns `nonce || ciphertext` (12 + len bytes).
pub fn encrypt(plaintext: &[u8], key: &Key) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key);
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a blob produced by [`encrypt`].
///
/// Expects `nonce (12 bytes) || ciphertext`.
pub fn decrypt(blob: &[u8], key: &Key) -> Result<Vec<u8>> {
    if blob.len() < NONCE_LEN {
        anyhow::bail!("ciphertext too short");
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed (wrong passphrase?): {e}"))
}

/// Derive a 256-bit master key from a passphrase and salt using Argon2id.
pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<Key> {
    use argon2::Argon2;

    let mut key_bytes = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase, salt, &mut key_bytes)
        .map_err(|e| anyhow::anyhow!("argon2 key derivation failed: {e}"))?;
    Ok(*Key::from_slice(&key_bytes))
}

/// Generate a random 256-bit data-encryption key (DEK).
pub fn generate_dek() -> Key {
    ChaCha20Poly1305::generate_key(&mut OsRng)
}

/// First-run key setup:
/// 1. Generate a random DEK.
/// 2. Derive a master key from the passphrase.
/// 3. Encrypt the DEK with the master key.
/// 4. Write `salt (16 bytes) || encrypted_dek` to `key_path`.
pub fn setup_key(passphrase: &[u8], key_path: &std::path::Path) -> Result<Key> {
    use rand::RngCore;

    let mut salt = [0u8; 16];
    rand::rng().fill_bytes(&mut salt);

    let master_key = derive_key(passphrase, &salt)?;
    let dek = generate_dek();
    let encrypted_dek = encrypt(dek.as_slice(), &master_key)?;

    let mut out = Vec::with_capacity(16 + encrypted_dek.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&encrypted_dek);

    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(key_path, &out).context("failed to write key file")?;

    Ok(dek)
}

/// Unlock the DEK using the passphrase and the key file written by [`setup_key`].
pub fn unlock(passphrase: &[u8], key_path: &std::path::Path) -> Result<Key> {
    let data = std::fs::read(key_path).context("failed to read key file")?;
    if data.len() < 16 {
        anyhow::bail!("key file is corrupt (too short)");
    }
    let (salt, encrypted_dek) = data.split_at(16);
    let master_key = derive_key(passphrase, salt)?;
    let dek_bytes = decrypt(encrypted_dek, &master_key)?;
    if dek_bytes.len() != 32 {
        anyhow::bail!("decrypted DEK has unexpected length");
    }
    Ok(*Key::from_slice(&dek_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = generate_dek();
        let plaintext = b"hello, recalld!";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = generate_dek();
        let key2 = generate_dek();
        let encrypted = encrypt(b"secret", &key1).unwrap();
        assert!(decrypt(&encrypted, &key2).is_err());
    }

    #[test]
    fn key_setup_and_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.enc");
        let passphrase = b"test-passphrase-123";

        let dek = setup_key(passphrase, &key_path).unwrap();
        let recovered = unlock(passphrase, &key_path).unwrap();
        assert_eq!(dek, recovered);

        // Wrong passphrase should fail
        assert!(unlock(b"wrong", &key_path).is_err());
    }
}
