//! The PairCam phone-to-desktop protocol. The specification is `docs/PROTOCOL.md`.
//!
//! This crate does no I/O of its own beyond reading and writing whole records on a blocking
//! stream. The desktop engine (the host) and the test simulator (a fake camera) both build on it.

mod channel;
mod error;
mod handshake;
mod message;
mod record;

pub use channel::{Opener, Sealer};
pub use error::{Error, Result};
pub use handshake::{
    CameraHandshake, CameraHello, CameraHelloRecord, DeviceId, HostHandshake, HostHello,
    HostHelloRecord, Mode, PairingKey, SessionKeys, Status, UNPAIRED,
};
pub use message::{
    Bye, CameraInfo, Capabilities, Codec, Configure, ControlCapabilities, ControlValues, FocusMode,
    Format, Gesture, GestureKind, GesturePhase, Hello, Lens, LogEntry, LogLevel, Message,
    PairResult, Point, Position, Range, Rect, Stabilization, Stats, Subject, Thermal, VideoConfig,
    VideoFrame, WhiteBalanceMode,
};
pub use record::{MAX_RECORD_LEN, read_record, write_record};

/// The protocol version this crate speaks.
pub const VERSION: u8 = 0;

/// The TCP port the camera listens on unless it had to pick another one.
pub const DEFAULT_PORT: u16 = 47810;

/// The Bonjour service type the camera advertises.
pub const SERVICE_TYPE: &str = "_paircam._tcp";

/// Fills an array with bytes from the operating system's random number generator.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("the operating system random number generator failed");
    bytes
}
