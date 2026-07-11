//! Length-prefixed framing for messages sent over the named pipe.
//!
//! Each frame is a 4-byte little-endian `u32` length prefix followed by
//! exactly that many bytes of payload. [`read_frame`] rejects an oversized
//! length prefix before allocating or reading any payload bytes, bounding
//! how much memory/data a misbehaving or malicious peer can make us buffer.

use std::io::{self, Read, Write};

/// Maximum allowed payload size for a single frame (64 KiB). IPC control
/// messages are small and fixed-shape; this is generous headroom, not a
/// tuning knob.
pub const MAX_FRAME_LEN: u32 = 64 * 1024;

/// Writes `bytes` as a single length-prefixed frame.
pub fn write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len: u32 = bytes.len().try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame payload too large to encode a u32 length prefix",
        )
    })?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(bytes)?;
    Ok(())
}

/// Reads a single length-prefixed frame, returning its payload bytes.
///
/// Rejects with an [`io::ErrorKind::InvalidData`] error if the encoded
/// length exceeds [`MAX_FRAME_LEN`] — this check happens before any
/// allocation or read of the payload itself.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);

    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds MAX_FRAME_LEN ({MAX_FRAME_LEN})"),
        ));
    }

    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_small_payload() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();

        let mut cursor = Cursor::new(buf);
        let payload = read_frame(&mut cursor).unwrap();
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn round_trips_near_max_payload() {
        let payload = vec![0xABu8; (MAX_FRAME_LEN - 1) as usize];
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload).unwrap();

        let mut cursor = Cursor::new(buf);
        let read_back = read_frame(&mut cursor).unwrap();
        assert_eq!(read_back, payload);
    }

    #[test]
    fn round_trips_exactly_max_payload() {
        let payload = vec![0x42u8; MAX_FRAME_LEN as usize];
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload).unwrap();

        let mut cursor = Cursor::new(buf);
        let read_back = read_frame(&mut cursor).unwrap();
        assert_eq!(read_back, payload);
    }

    #[test]
    fn oversized_length_prefix_is_rejected_before_reading_payload() {
        // Craft a frame whose length prefix claims far more than
        // MAX_FRAME_LEN, but whose actual buffer is tiny — if read_frame
        // tried to allocate/read that many bytes first, this would fail
        // with an EOF/UnexpectedEof rather than our InvalidData rejection.
        let huge_len: u32 = u32::MAX;
        let mut buf = Vec::new();
        buf.extend_from_slice(&huge_len.to_le_bytes());
        // Deliberately no payload bytes follow.

        let mut cursor = Cursor::new(buf);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_stream_returns_error_not_panic() {
        // A length prefix claiming 10 bytes, but only 3 are actually present.
        let mut buf = Vec::new();
        buf.extend_from_slice(&10u32.to_le_bytes());
        buf.extend_from_slice(&[1, 2, 3]);

        let mut cursor = Cursor::new(buf);
        let result = read_frame(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_length_prefix_returns_error_not_panic() {
        let buf = vec![0u8, 1]; // only 2 of 4 length-prefix bytes
        let mut cursor = Cursor::new(buf);
        let result = read_frame(&mut cursor);
        assert!(result.is_err());
    }
}
