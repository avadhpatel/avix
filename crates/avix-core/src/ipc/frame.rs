use crate::error::AvixError;
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, AvixError> {
    let body = serde_json::to_vec(msg).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(AvixError::ConfigParse("message exceeds 16 MB limit".into()));
    }
    let len = (body.len() as u32).to_le_bytes();
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&len);
    buf.extend_from_slice(&body);
    Ok(buf)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, AvixError> {
    if bytes.len() < 4 {
        return Err(AvixError::ConfigParse("frame too short".into()));
    }
    let body = &bytes[4..];
    serde_json::from_slice(body).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn read_from<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T, AvixError> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(AvixError::ConfigParse("frame too large".into()));
    }
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn write_to<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    msg: &T,
) -> Result<(), AvixError> {
    let bytes = encode(msg)?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))
}
