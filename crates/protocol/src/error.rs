use crate::handshake::Status;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),

    #[error("record of {0} bytes is larger than the limit")]
    RecordTooLarge(usize),

    #[error("the peer is not speaking the PairCam protocol")]
    BadMagic,

    #[error("the peer speaks protocol version {0}, which this app does not support")]
    UnsupportedVersion(u8),

    #[error("malformed {0}")]
    Malformed(&'static str),

    #[error("the camera refused the connection: {0:?}")]
    Refused(Status),

    #[error("the host key does not match its commitment")]
    CommitmentMismatch,

    #[error("the key exchange produced an invalid shared secret")]
    InvalidKeyExchange,

    #[error("a record failed to decrypt")]
    Decrypt,

    #[error("the record counter ran out")]
    CounterExhausted,

    #[error("invalid JSON in {message}: {source}")]
    Json {
        message: &'static str,
        source: serde_json::Error,
    },
}
