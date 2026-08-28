//! Length-prefixed MessagePack framing: `[u32 LE length][rmp payload]`.

use crate::protocol::MAX_FRAME_LEN;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn invalid_data<E>(e: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Refuse any frame over `MAX_FRAME_LEN` before it is written or allocated —
/// the cap is the only thing standing between a corrupt header and a
/// multi-gigabyte `vec![0u8; len]`.
fn check_len(len: u32) -> io::Result<()> {
    if len > MAX_FRAME_LEN {
        return Err(invalid_data("frame too large"));
    }
    Ok(())
}

pub async fn write_frame<T, W>(w: &mut W, msg: &T) -> io::Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let payload = rmp_serde::to_vec(msg).map_err(invalid_data)?;
    let len = payload.len() as u32;
    check_len(len)?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(&payload).await?;
    w.flush().await
}

/// Returns Ok(None) on clean EOF at a frame boundary.
pub async fn read_frame<T, R>(r: &mut R) -> io::Result<Option<T>>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf);
    check_len(len)?;
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    rmp_serde::from_slice(&payload)
        .map(Some)
        .map_err(invalid_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &("hello".to_string(), 7u32))
            .await
            .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let got: Option<(String, u32)> = read_frame(&mut cursor).await.unwrap();
        assert_eq!(got, Some(("hello".to_string(), 7)));
        let eof: Option<(String, u32)> = read_frame(&mut cursor).await.unwrap();
        assert_eq!(eof, None, "clean EOF at a frame boundary is Ok(None)");
    }

    #[tokio::test]
    async fn oversized_frames_are_refused_on_both_sides() {
        let big = vec![0u8; MAX_FRAME_LEN as usize + 1];
        let mut sink = Vec::new();
        let err = write_frame(&mut sink, &serde_bytes::ByteBuf::from(big))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("frame too large"), "got: {err}");
        assert!(sink.is_empty(), "nothing is written for a refused frame");

        let header = (MAX_FRAME_LEN + 1).to_le_bytes();
        let mut cursor = std::io::Cursor::new(header.to_vec());
        let err = read_frame::<Vec<u8>, _>(&mut cursor).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("frame too large"), "got: {err}");
    }
}
