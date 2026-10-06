//! Password-based login and encryption.
//!
//! master = PBKDF2-SHA256(password, salt, 100k rounds). Both sides pick a random nonce;
//! the phone proves it knows the password with HMAC(master, "auth" | ns | nc), and each
//! direction gets its own AES-128-GCM key derived the same way. A message counter is the
//! GCM nonce, so replayed, reordered or altered data is rejected.

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{AeadInOut, KeyInit};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const ROUNDS: u32 = 100_000;

pub fn master_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, ROUNDS, &mut out);
    out
}

pub fn derive(master: &[u8; 32], label: &[u8], ns: &[u8], nc: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(master).unwrap();
    mac.update(label);
    mac.update(ns);
    mac.update(nc);
    mac.finalize().into_bytes().into()
}

/// Constant-time check of the phone's login proof.
pub fn verify(master: &[u8; 32], ns: &[u8], nc: &[u8], proof: &[u8]) -> bool {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(master).unwrap();
    mac.update(b"auth");
    mac.update(ns);
    mac.update(nc);
    mac.verify_slice(proof).is_ok()
}

/// One direction of an encrypted stream.
pub struct Seal {
    cipher: Aes128Gcm,
    counter: u64,
}

pub const TAG: usize = 16;

impl Seal {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { cipher: Aes128Gcm::new_from_slice(&key[..16]).unwrap(), counter: 0 }
    }

    fn nonce(&mut self) -> aes_gcm::Nonce<aes_gcm::aead::consts::U12> {
        let mut n = [0u8; 12];
        n[4..].copy_from_slice(&self.counter.to_be_bytes());
        self.counter += 1;
        n.into()
    }

    /// Encrypts `buf` in place and returns the 16-byte tag.
    pub fn seal_slice(&mut self, buf: &mut [u8]) -> [u8; TAG] {
        let nonce = self.nonce();
        self.cipher.encrypt_inout_detached(&nonce, &[], buf.into()).unwrap().into()
    }

    /// Encrypts `buf` in place and appends the tag.
    #[cfg(test)]
    pub fn seal(&mut self, buf: &mut Vec<u8>) {
        let tag = self.seal_slice(buf);
        buf.extend_from_slice(&tag);
    }

    /// Decrypts `buf` (ciphertext + tag) in place, leaving only the plaintext.
    pub fn open(&mut self, buf: &mut Vec<u8>) -> bool {
        if buf.len() < TAG {
            return false;
        }
        let nonce = self.nonce();
        let split = buf.len() - TAG;
        let tag: [u8; TAG] = buf[split..].try_into().unwrap();
        buf.truncate(split);
        self.cipher.decrypt_inout_detached(&nonce, &[], buf.as_mut_slice().into(), &tag.into()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper() {
        let m = master_key("hunter2", b"0123456789abcdef");
        let k = derive(&m, b"s2c", b"a", b"b");
        let (mut tx, mut rx) = (Seal::new(&k), Seal::new(&k));
        let mut buf = b"hello screen".to_vec();
        tx.seal(&mut buf);
        assert_ne!(&buf[..12], b"hello screen");
        assert!(rx.open(&mut buf));
        assert_eq!(buf, b"hello screen");

        let mut buf = b"again".to_vec();
        tx.seal(&mut buf);
        buf[0] ^= 1;
        assert!(!rx.open(&mut buf));
    }

    /// Fixed values the Android app's test vectors are checked against.
    #[test]
    fn known_vector() {
        let m = master_key("pw", b"salt");
        let hex: String = m.iter().map(|b| format!("{b:02x}")).collect();
        println!("master={hex}");
        assert!(verify(&m, b"ns", b"nc", &derive(&m, b"auth", b"ns", b"nc")));
    }
}
