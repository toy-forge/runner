//! The base protocol's framing: a `Content-Length` header, a blank line, then the JSON body.

use std::io;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Writes `message` as one framed JSON-RPC message.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

/// Reads one framed message. `Ok(None)` at the end of the stream.
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse().ok();
        }
    }
    let length = length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_a_message() {
        let message = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"text": "héllo"}});
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).await.unwrap();
        let mut reader = BufReader::new(buffer.as_slice());
        assert_eq!(read_message(&mut reader).await.unwrap(), Some(message));
        assert_eq!(read_message(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn counts_bytes_not_characters() {
        let body = r#"{"a":"é"}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut reader = BufReader::new(framed.as_bytes());
        assert_eq!(
            read_message(&mut reader).await.unwrap(),
            Some(serde_json::json!({"a": "é"}))
        );
    }
}
