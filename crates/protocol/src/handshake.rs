//! The three-record handshake and the keys it derives. See "Handshake" in `docs/PROTOCOL.md`.

use std::fmt;

use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::{Error, Result, VERSION};

const MAGIC: &[u8; 4] = b"PCAM";
const HOST_HELLO_LEN: usize = 4 + 1 + 1 + 16 + 32;
const CAMERA_HELLO_LEN: usize = 4 + 1 + 1 + 16 + 32;
const HOST_KEY_LEN: usize = 32;

/// The key two paired devices share. It is mixed into the session keys when resuming.
pub type PairingKey = [u8; 32];

/// The value used in place of a pairing key while pairing for the first time.
pub const UNPAIRED: PairingKey = [0u8; 32];

/// A device's permanent random identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
    pub fn random() -> Self {
        Self(crate::random_bytes())
    }

    /// Parses the 32 hex characters used in discovery records and settings files.
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DeviceId({})", self.to_hex())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// Whether the host wants to pair for the first time or reconnect to a paired camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Pair,
    Resume,
}

/// The camera's answer to a connection attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    UnsupportedVersion,
    NotPaired,
    Busy,
    Other(u8),
}

impl Status {
    fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Ok,
            1 => Self::UnsupportedVersion,
            2 => Self::NotPaired,
            3 => Self::Busy,
            other => Self::Other(other),
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::UnsupportedVersion => 1,
            Self::NotPaired => 2,
            Self::Busy => 3,
            Self::Other(other) => other,
        }
    }
}

/// Record 1, as the camera reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostHello {
    pub version: u8,
    pub mode: Mode,
    pub host_id: DeviceId,
    pub commitment: [u8; 32],
}

/// Record 2, as the host reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraHello {
    pub version: u8,
    pub status: Status,
    pub camera_id: DeviceId,
    pub public_key: [u8; 32],
}

/// Everything both sides derive from a finished handshake.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionKeys {
    /// Encrypts records the host sends.
    pub host_to_camera: [u8; 32],
    /// Encrypts records the camera sends.
    pub camera_to_host: [u8; 32],
    /// Stored by both sides after a successful pairing, then used to resume.
    pub pairing_key: PairingKey,
    /// The number both apps show while pairing, below 1 000 000.
    pub pairing_code: u32,
}

impl SessionKeys {
    /// The pairing code as the six digits shown to the user.
    pub fn pairing_code_digits(&self) -> String {
        format!("{:06}", self.pairing_code)
    }
}

impl fmt::Debug for SessionKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionKeys(..)")
    }
}

fn derive_keys(
    private_key: &StaticSecret,
    peer_public_key: &[u8; 32],
    pairing_key: &PairingKey,
    records: [&[u8]; 3],
) -> Result<SessionKeys> {
    let shared = private_key.diffie_hellman(&PublicKey::from(*peer_public_key));
    if !shared.was_contributory() {
        return Err(Error::InvalidKeyExchange);
    }

    let mut transcript = Sha256::new();
    for record in records {
        transcript.update(record);
    }
    let transcript = transcript.finalize();

    let hkdf = Hkdf::<Sha256>::new(Some(pairing_key), shared.as_bytes());
    let expand = |label: &[u8], output: &mut [u8]| {
        hkdf.expand_multi_info(&[label, &transcript], output)
            .expect("the output is shorter than the HKDF limit");
    };

    let mut keys = SessionKeys {
        host_to_camera: [0; 32],
        camera_to_host: [0; 32],
        pairing_key: [0; 32],
        pairing_code: 0,
    };
    expand(b"paircam v0 h2c", &mut keys.host_to_camera);
    expand(b"paircam v0 c2h", &mut keys.camera_to_host);
    expand(b"paircam v0 pairing", &mut keys.pairing_key);
    let mut code = [0u8; 4];
    expand(b"paircam v0 sas", &mut code);
    keys.pairing_code = u32::from_be_bytes(code) % 1_000_000;
    Ok(keys)
}

/// The host side of the handshake.
///
/// ```text
/// let handshake = HostHandshake::new(host_id, mode, random_bytes());
/// send(handshake.hello());
/// let camera = HostHandshake::read_camera_hello(&receive())?;
/// let (host_key, keys) = handshake.finish(&camera, &pairing_key_for(camera.camera_id))?;
/// send(host_key);
/// ```
pub struct HostHandshake {
    private_key: StaticSecret,
    public_key: [u8; 32],
    hello: Vec<u8>,
}

impl HostHandshake {
    /// Starts a handshake. `private_key` must be 32 fresh random bytes.
    pub fn new(host_id: DeviceId, mode: Mode, private_key: [u8; 32]) -> Self {
        let private_key = StaticSecret::from(private_key);
        let public_key = PublicKey::from(&private_key).to_bytes();

        let mut hello = Vec::with_capacity(HOST_HELLO_LEN);
        hello.extend_from_slice(MAGIC);
        hello.push(VERSION);
        hello.push(match mode {
            Mode::Pair => 0,
            Mode::Resume => 1,
        });
        hello.extend_from_slice(&host_id.0);
        hello.extend_from_slice(&Sha256::digest(public_key));

        Self {
            private_key,
            public_key,
            hello,
        }
    }

    /// The body of record 1.
    pub fn hello(&self) -> &[u8] {
        &self.hello
    }

    /// Parses record 2. Fails if the camera refused the connection.
    pub fn read_camera_hello(body: &[u8]) -> Result<CameraHelloRecord> {
        if body.len() < 4 || &body[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        if body.len() != CAMERA_HELLO_LEN {
            return Err(Error::Malformed("camera hello"));
        }
        let hello = CameraHello {
            version: body[4],
            status: Status::from_byte(body[5]),
            camera_id: DeviceId(body[6..22].try_into().expect("length checked")),
            public_key: body[22..54].try_into().expect("length checked"),
        };
        if hello.status != Status::Ok {
            return Err(Error::Refused(hello.status));
        }
        if hello.version != VERSION {
            return Err(Error::UnsupportedVersion(hello.version));
        }
        Ok(CameraHelloRecord {
            hello,
            body: body.to_vec(),
        })
    }

    /// Finishes the handshake. Returns the body of record 3 and the session keys.
    ///
    /// `pairing_key` is [`UNPAIRED`] in pair mode and the stored key in resume mode.
    pub fn finish(
        self,
        camera: &CameraHelloRecord,
        pairing_key: &PairingKey,
    ) -> Result<(Vec<u8>, SessionKeys)> {
        let host_key = self.public_key.to_vec();
        let keys = derive_keys(
            &self.private_key,
            &camera.hello.public_key,
            pairing_key,
            [&self.hello, &camera.body, &host_key],
        )?;
        Ok((host_key, keys))
    }
}

/// A parsed record 2 together with its exact bytes, which the key derivation needs.
pub struct CameraHelloRecord {
    hello: CameraHello,
    body: Vec<u8>,
}

impl std::ops::Deref for CameraHelloRecord {
    type Target = CameraHello;

    fn deref(&self) -> &CameraHello {
        &self.hello
    }
}

/// The camera side of the handshake. The desktop uses it only in tests and the simulator.
///
/// ```text
/// let host = CameraHandshake::read_host_hello(&receive())?;
/// let handshake = CameraHandshake::accept(&host, camera_id, random_bytes());
/// send(handshake.hello());
/// let keys = handshake.finish(&receive(), &pairing_key_for(host.host_id))?;
/// ```
pub struct CameraHandshake {
    private_key: StaticSecret,
    host_hello: Vec<u8>,
    commitment: [u8; 32],
    hello: Vec<u8>,
}

/// A parsed record 1 together with its exact bytes.
pub struct HostHelloRecord {
    hello: HostHello,
    body: Vec<u8>,
}

impl std::ops::Deref for HostHelloRecord {
    type Target = HostHello;

    fn deref(&self) -> &HostHello {
        &self.hello
    }
}

fn camera_hello_body(status: Status, camera_id: DeviceId, public_key: &[u8; 32]) -> Vec<u8> {
    let mut hello = Vec::with_capacity(CAMERA_HELLO_LEN);
    hello.extend_from_slice(MAGIC);
    hello.push(VERSION);
    hello.push(status.to_byte());
    hello.extend_from_slice(&camera_id.0);
    hello.extend_from_slice(public_key);
    hello
}

impl CameraHandshake {
    /// Parses record 1. The version is not checked here so that the camera can answer an
    /// unsupported version with a refusal the host understands.
    pub fn read_host_hello(body: &[u8]) -> Result<HostHelloRecord> {
        if body.len() < 4 || &body[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        if body.len() < 6 {
            return Err(Error::Malformed("host hello"));
        }
        let version = body[4];
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        if body.len() != HOST_HELLO_LEN {
            return Err(Error::Malformed("host hello"));
        }
        let mode = match body[5] {
            0 => Mode::Pair,
            1 => Mode::Resume,
            _ => return Err(Error::Malformed("host hello mode")),
        };
        Ok(HostHelloRecord {
            hello: HostHello {
                version,
                mode,
                host_id: DeviceId(body[6..22].try_into().expect("length checked")),
                commitment: body[22..54].try_into().expect("length checked"),
            },
            body: body.to_vec(),
        })
    }

    /// The body of a record 2 that turns the host away.
    pub fn refusal(status: Status, camera_id: DeviceId) -> Vec<u8> {
        camera_hello_body(status, camera_id, &[0u8; 32])
    }

    /// Accepts the host. `private_key` must be 32 fresh random bytes.
    pub fn accept(host: &HostHelloRecord, camera_id: DeviceId, private_key: [u8; 32]) -> Self {
        let private_key = StaticSecret::from(private_key);
        let public_key = PublicKey::from(&private_key).to_bytes();
        Self {
            private_key,
            host_hello: host.body.clone(),
            commitment: host.hello.commitment,
            hello: camera_hello_body(Status::Ok, camera_id, &public_key),
        }
    }

    /// The body of record 2.
    pub fn hello(&self) -> &[u8] {
        &self.hello
    }

    /// Finishes the handshake with record 3 from the host.
    ///
    /// `pairing_key` is [`UNPAIRED`] in pair mode and the stored key in resume mode.
    pub fn finish(self, host_key: &[u8], pairing_key: &PairingKey) -> Result<SessionKeys> {
        let host_public_key: [u8; HOST_KEY_LEN] = host_key
            .try_into()
            .map_err(|_| Error::Malformed("host key"))?;
        if Sha256::digest(host_public_key).as_slice() != self.commitment {
            return Err(Error::CommitmentMismatch);
        }
        derive_keys(
            &self.private_key,
            &host_public_key,
            pairing_key,
            [&self.host_hello, &self.hello, host_key],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_ID: DeviceId = DeviceId([0x11; 16]);
    const CAMERA_ID: DeviceId = DeviceId([0x22; 16]);

    fn run(
        mode: Mode,
        host_pairing_key: &PairingKey,
        camera_pairing_key: &PairingKey,
    ) -> (SessionKeys, SessionKeys) {
        let host = HostHandshake::new(HOST_ID, mode, [0x33; 32]);

        let host_hello = CameraHandshake::read_host_hello(host.hello()).unwrap();
        assert_eq!(host_hello.mode, mode);
        assert_eq!(host_hello.host_id, HOST_ID);
        let camera = CameraHandshake::accept(&host_hello, CAMERA_ID, [0x44; 32]);

        let camera_hello = HostHandshake::read_camera_hello(camera.hello()).unwrap();
        assert_eq!(camera_hello.camera_id, CAMERA_ID);
        let (host_key, host_keys) = host.finish(&camera_hello, host_pairing_key).unwrap();

        let camera_keys = camera.finish(&host_key, camera_pairing_key).unwrap();
        (host_keys, camera_keys)
    }

    #[test]
    fn both_sides_derive_the_same_keys_when_pairing() {
        let (host, camera) = run(Mode::Pair, &UNPAIRED, &UNPAIRED);
        assert_eq!(host, camera);
        assert_ne!(host.host_to_camera, host.camera_to_host);
        assert!(host.pairing_code < 1_000_000);
        assert_eq!(host.pairing_code_digits().len(), 6);
    }

    #[test]
    fn resuming_with_the_stored_key_matches() {
        let (paired, _) = run(Mode::Pair, &UNPAIRED, &UNPAIRED);
        let (host, camera) = run(Mode::Resume, &paired.pairing_key, &paired.pairing_key);
        assert_eq!(host, camera);
        assert_ne!(host.host_to_camera, paired.host_to_camera);
    }

    #[test]
    fn resuming_with_the_wrong_key_gives_different_keys() {
        let (host, camera) = run(Mode::Resume, &[1; 32], &[2; 32]);
        assert_ne!(host.host_to_camera, camera.host_to_camera);
        assert_ne!(host.camera_to_host, camera.camera_to_host);
    }

    #[test]
    fn camera_rejects_a_host_key_that_breaks_the_commitment() {
        let host = HostHandshake::new(HOST_ID, Mode::Pair, [0x33; 32]);
        let host_hello = CameraHandshake::read_host_hello(host.hello()).unwrap();
        let camera = CameraHandshake::accept(&host_hello, CAMERA_ID, [0x44; 32]);

        let other_key = PublicKey::from(&StaticSecret::from([0x55; 32])).to_bytes();
        assert!(matches!(
            camera.finish(&other_key, &UNPAIRED),
            Err(Error::CommitmentMismatch)
        ));
    }

    #[test]
    fn host_reports_a_refusal() {
        let refusal = CameraHandshake::refusal(Status::NotPaired, CAMERA_ID);
        assert!(matches!(
            HostHandshake::read_camera_hello(&refusal),
            Err(Error::Refused(Status::NotPaired))
        ));
    }

    #[test]
    fn rejects_foreign_and_truncated_records() {
        assert!(matches!(
            CameraHandshake::read_host_hello(b"GET / HTTP/1.1"),
            Err(Error::BadMagic)
        ));
        assert!(matches!(
            HostHandshake::read_camera_hello(b"PCAM\x00\x00"),
            Err(Error::Malformed(_))
        ));
        let mut future = HostHandshake::new(HOST_ID, Mode::Pair, [0x33; 32])
            .hello()
            .to_vec();
        future[4] = 9;
        assert!(matches!(
            CameraHandshake::read_host_hello(&future),
            Err(Error::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn device_ids_round_trip_through_hex() {
        let id = DeviceId([0xab; 16]);
        assert_eq!(DeviceId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(DeviceId::from_hex("abc"), None);
        assert_eq!(DeviceId::from_hex(&"zz".repeat(16)), None);
    }
}
