//! Login and encryption.
//!
//! Each trusted phone shares a 32-byte device key with the PC (made during pairing).
//! Both sides pick a random nonce; the phone proves it has the key with
//! HMAC(key, "auth" | ns | nc), and each
//! direction gets its own AES-128-GCM key derived the same way. A message counter is the
//! GCM nonce, so replayed, reordered or altered data is rejected.

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{AeadInOut, KeyInit};
use hmac::{Hmac, Mac};
use sha2::Sha256;

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
        let m = [3u8; 32];
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

    #[test]
    fn proof() {
        let m = [1u8; 32];
        assert!(verify(&m, b"ns", b"nc", &derive(&m, b"auth", b"ns", b"nc")));
        assert!(!verify(&[2u8; 32], b"ns", b"nc", &derive(&m, b"auth", b"ns", b"nc")));
    }
}
