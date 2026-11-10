use std::io::{Read, Write};

use crate::{Error, Result};

/// The largest record body either side accepts.
pub const MAX_RECORD_LEN: usize = 8 * 1024 * 1024;

/// Writes one record: a `u32` length followed by the body.
pub fn write_record(writer: &mut impl Write, body: &[u8]) -> Result<()> {
    if body.len() > MAX_RECORD_LEN {
        return Err(Error::RecordTooLarge(body.len()));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(body)?;
    Ok(())
}

/// Reads one record body, blocking until all of it has arrived.
pub fn read_record(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_RECORD_LEN {
        return Err(Error::RecordTooLarge(length));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_records() {
        let mut wire = Vec::new();
        write_record(&mut wire, b"first").unwrap();
        write_record(&mut wire, b"").unwrap();
        write_record(&mut wire, b"third").unwrap();

        let mut reader = wire.as_slice();
        assert_eq!(read_record(&mut reader).unwrap(), b"first");
        assert_eq!(read_record(&mut reader).unwrap(), b"");
        assert_eq!(read_record(&mut reader).unwrap(), b"third");
        assert!(matches!(read_record(&mut reader), Err(Error::Io(_))));
    }

    #[test]
    fn rejects_oversized_lengths() {
        let wire = ((MAX_RECORD_LEN + 1) as u32).to_be_bytes();
        assert!(matches!(
            read_record(&mut wire.as_slice()),
            Err(Error::RecordTooLarge(_))
        ));
    }
}
