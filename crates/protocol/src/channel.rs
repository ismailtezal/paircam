use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};

use crate::{Error, Result};

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(bytes)
}

/// Encrypts the records one side sends.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            counter: 0,
        }
    }

    /// Encrypts `message` in place, turning it into a record body.
    pub fn seal(&mut self, message: &mut Vec<u8>) -> Result<()> {
        let counter = self.counter;
        self.counter = counter.checked_add(1).ok_or(Error::CounterExhausted)?;
        self.cipher
            .encrypt_in_place(&nonce(counter), b"", message)
            .map_err(|_| Error::RecordTooLarge(message.len()))
    }
}

/// Decrypts the records one side receives.
pub struct Opener {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Opener {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            counter: 0,
        }
    }

    /// Decrypts a record body in place, leaving the message.
    pub fn open(&mut self, record: &mut Vec<u8>) -> Result<()> {
        let counter = self.counter;
        self.counter = counter.checked_add(1).ok_or(Error::CounterExhausted)?;
        self.cipher
            .decrypt_in_place(&nonce(counter), b"", record)
            .map_err(|_| Error::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_in_order() {
        let key = [7u8; 32];
        let mut sealer = Sealer::new(&key);
        let mut opener = Opener::new(&key);

        for text in [&b"one"[..], b"", b"three"] {
            let mut buffer = text.to_vec();
            sealer.seal(&mut buffer).unwrap();
            assert_eq!(buffer.len(), text.len() + 16);
            opener.open(&mut buffer).unwrap();
            assert_eq!(buffer, text);
        }
    }

    #[test]
    fn rejects_replayed_and_tampered_records() {
        let key = [7u8; 32];
        let mut sealer = Sealer::new(&key);
        let mut opener = Opener::new(&key);

        let mut first = b"first".to_vec();
        sealer.seal(&mut first).unwrap();
        let replay = first.clone();
        opener.open(&mut first).unwrap();
        assert!(matches!(
            opener.open(&mut replay.clone()),
            Err(Error::Decrypt)
        ));

        let mut opener = Opener::new(&key);
        let mut tampered = replay;
        tampered[0] ^= 1;
        assert!(matches!(opener.open(&mut tampered), Err(Error::Decrypt)));
    }

    #[test]
    fn rejects_the_wrong_key() {
        let mut record = b"secret".to_vec();
        Sealer::new(&[1u8; 32]).seal(&mut record).unwrap();
        assert!(matches!(
            Opener::new(&[2u8; 32]).open(&mut record),
            Err(Error::Decrypt)
        ));
    }
}
