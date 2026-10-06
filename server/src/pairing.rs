//! "Trust this phone" pairing, like Bluetooth numeric comparison.
//!
//! 1. phone → PC: commitment = SHA256(phone public key), plus the phone's name
//! 2. PC → phone: PC public key
//! 3. phone → PC: phone public key (PC checks it matches the commitment)
//! 4. Both show a 6-digit code made from both public keys; the user checks they match and
//!    clicks Allow on the PC. Both then derive the same long-term device key from the
//!    P-256 shared secret. The phone committing to its key first stops a man-in-the-middle
//!    from searching for keys that make the codes match.

use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

pub struct KeyPair {
    secret: SecretKey,
    /// Uncompressed SEC1 public key (65 bytes).
    pub public: Vec<u8>,
}

impl KeyPair {
    pub fn new() -> Self {
        loop {
            let mut b = [0u8; 32];
            getrandom::fill(&mut b).expect("no random source");
            if let Ok(secret) = SecretKey::from_slice(&b) {
                let public = secret.public_key().to_sec1_point(false).as_bytes().to_vec();
                return Self { secret, public };
            }
        }
    }

    /// Long-term device key shared with the other side, or `None` if their key is invalid.
    pub fn device_key(&self, theirs: &[u8], phone_pub: &[u8], pc_pub: &[u8]) -> Option<[u8; 32]> {
        let their = PublicKey::from_sec1_bytes(theirs).ok()?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), their.as_affine());
        let mut h = Sha256::new();
        h.update(b"EzRDP device key v1");
        h.update(shared.raw_secret_bytes());
        h.update(phone_pub);
        h.update(pc_pub);
        Some(h.finalize().into())
    }
}

pub fn commit(phone_pub: &[u8]) -> [u8; 32] {
    Sha256::digest(phone_pub).into()
}

/// The 6-digit code both screens show.
pub fn code(phone_pub: &[u8], pc_pub: &[u8]) -> u32 {
    let mut h = Sha256::new();
    h.update(b"EzRDP code v1");
    h.update(phone_pub);
    h.update(pc_pub);
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000
}

pub fn format_code(c: u32) -> String {
    format!("{:03} {:03}", c / 1000, c % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_agree() {
        let (phone, pc) = (KeyPair::new(), KeyPair::new());
        assert_eq!(phone.public.len(), 65);
        let a = phone.device_key(&pc.public, &phone.public, &pc.public).unwrap();
        let b = pc.device_key(&phone.public, &phone.public, &pc.public).unwrap();
        assert_eq!(a, b);
        assert!(pc.device_key(&[4; 65], &phone.public, &pc.public).is_none());
        assert_eq!(format_code(7042), "007 042");
    }
}
