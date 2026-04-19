use crate::error::AvixError;
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::instrument;

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[instrument]
pub fn encode<T: Serialize + std::fmt::Debug>(msg: &T) -> Result<Vec<u8>, AvixError> {
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

#[instrument]
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, AvixError> {
    if bytes.len() < 4 {
        return Err(AvixError::ConfigParse("frame too short".into()));
    }
    let body = &bytes[4..];
    serde_json::from_slice(body).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

#[instrument]
pub async fn read_from<R: AsyncRead + Unpin + std::fmt::Debug, T: DeserializeOwned>(
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

#[instrument]
pub async fn write_to<W: AsyncWrite + Unpin + std::fmt::Debug, T: Serialize + std::fmt::Debug>(
    writer: &mut W,
    msg: &T,
) -> Result<(), AvixError> {
    let bytes = encode(msg)?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[instrument]
    #[test]
    fn test_encode_then_decode_roundtrip() {
        let msg = json!({"hello": "world", "count": 42});
        let encoded = encode(&msg).unwrap();
        // First 4 bytes are the length
        assert!(encoded.len() >= 4);
        let decoded: serde_json::Value = decode(&encoded).unwrap();
        assert_eq!(decoded["hello"], "world");
        assert_eq!(decoded["count"], 42);
    }

    #[instrument]
    #[test]
    fn test_decode_too_short_returns_error() {
        let short = &[1u8, 2, 3]; // only 3 bytes, need at least 4
        let result: Result<serde_json::Value, _> = decode(short);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("frame too short"), "got: {err}");
    }

    #[instrument]
    #[test]
    fn test_decode_invalid_json_body_returns_error() {
        // Build a frame with correct length prefix but invalid JSON body
        let body = b"not-valid-json!!!";
        let len = (body.len() as u32).to_le_bytes();
        let mut frame = Vec::new();
        frame.extend_from_slice(&len);
        frame.extend_from_slice(body);

        let result: Result<serde_json::Value, _> = decode(&frame);
        assert!(result.is_err());
    }

    #[instrument]
    #[test]
    fn test_encode_simple_struct() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Msg {
            id: u32,
            name: String,
        }
        let msg = Msg {
            id: 7,
            name: "test".into(),
        };
        let encoded = encode(&msg).unwrap();
        let decoded: Msg = decode(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[instrument]
    #[tokio::test]
    async fn test_write_to_and_read_from_cursor() {
        use tokio::io::BufReader;
        let msg = json!({"method": "test/call", "id": "1"});
        let mut buf = Vec::new();
        write_to(&mut buf, &msg).await.unwrap();

        // read_from expects an AsyncRead — use a cursor
        let mut reader = BufReader::new(std::io::Cursor::new(buf));
        let decoded: serde_json::Value = read_from(&mut reader).await.unwrap();
        assert_eq!(decoded["method"], "test/call");
    }

    #[instrument]
    #[tokio::test]
    async fn test_read_from_invalid_json_returns_error() {
        use tokio::io::BufReader;
        // Create a frame with a valid length prefix but garbage body
        let body = b"garbage!!!";
        let len = (body.len() as u32).to_le_bytes();
        let mut frame = Vec::new();
        frame.extend_from_slice(&len);
        frame.extend_from_slice(body);

        let mut reader = BufReader::new(std::io::Cursor::new(frame));
        let result: Result<serde_json::Value, _> = read_from(&mut reader).await;
        assert!(result.is_err());
    }
}
