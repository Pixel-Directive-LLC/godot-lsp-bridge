//! JSON-RPC message framing for the Language Server Protocol.
//!
//! LSP messages use HTTP-style headers to delimit payloads:
//!
//! ```text
//! Content-Length: <N>\r\n
//! \r\n
//! <N bytes of JSON>
//! ```
//!
//! This module provides [`read_message`] and [`write_message`] to encode and
//! decode these frames over any async buffered reader / async writer.

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// Maximum permitted message body size (64 MiB).
///
/// Protects against runaway allocations from malformed `Content-Length` values.
pub const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Reads one complete LSP message from `reader`.
///
/// Parses LSP/HTTP-style headers to locate the `Content-Length` field, then
/// reads exactly that many bytes as the message body.  Returns `Ok(None)` on
/// clean EOF before any header bytes are consumed.
///
/// # Errors
///
/// Returns an error if:
/// - Headers are malformed or `Content-Length` is absent
/// - The declared body length exceeds [`MAX_MESSAGE_SIZE`]
/// - An I/O error occurs mid-read
pub async fn read_message<R>(reader: &mut R) -> Result<Option<Vec<u8>>>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut saw_any = false;

    // Parse headers line-by-line until the blank separator line.
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("failed to read LSP header line")?;

        if n == 0 {
            // EOF — only valid before the first header byte.
            if saw_any {
                bail!("unexpected EOF while reading LSP headers");
            }
            return Ok(None);
        }

        saw_any = true;

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            // Blank line signals end of headers.
            break;
        }

        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            let len: usize = rest
                .trim()
                .parse()
                .context("invalid Content-Length value")?;
            content_length = Some(len);
        }
        // Other headers (e.g. Content-Type) are ignored per the LSP spec.
    }

    let len = content_length.context("LSP message missing Content-Length header")?;
    if len > MAX_MESSAGE_SIZE {
        bail!("Content-Length {len} exceeds maximum allowed size {MAX_MESSAGE_SIZE}");
    }

    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .context("failed to read LSP message body")?;

    Ok(Some(body))
}

/// Writes one LSP message to `writer`.
///
/// Prepends a `Content-Length` header followed by `\r\n\r\n`, writes `body`,
/// and flushes before returning.
///
/// # Errors
///
/// Returns an error if the underlying write or flush fails.
pub async fn write_message<W>(writer: &mut W, body: &[u8]) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .await
        .context("failed to write LSP header")?;
    writer
        .write_all(body)
        .await
        .context("failed to write LSP body")?;
    writer
        .flush()
        .await
        .context("failed to flush LSP message")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// Build a raw LSP frame from a body slice.
    fn make_frame(body: &[u8]) -> Vec<u8> {
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body);
        frame
    }

    #[tokio::test]
    async fn round_trip_simple() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let frame = make_frame(body);
        let mut reader = BufReader::new(frame.as_slice());
        let got = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let mut reader = BufReader::new(&b""[..]);
        let result = read_message(&mut reader).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn two_messages_then_eof() {
        // Simulates two back-to-back (batched) messages in one buffer.
        let body1 = br#"{"id":1}"#;
        let body2 = br#"{"id":2}"#;
        let mut data = make_frame(body1);
        data.extend(make_frame(body2));

        let mut reader = BufReader::new(data.as_slice());
        let m1 = read_message(&mut reader).await.unwrap().unwrap();
        let m2 = read_message(&mut reader).await.unwrap().unwrap();
        let eof = read_message(&mut reader).await.unwrap();

        assert_eq!(m1, body1);
        assert_eq!(m2, body2);
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn oversized_message_rejected() {
        let big_len = MAX_MESSAGE_SIZE + 1;
        let header = format!("Content-Length: {big_len}\r\n\r\n");
        let mut reader = BufReader::new(header.as_bytes());
        let err = read_message(&mut reader).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn missing_content_length_rejected() {
        let data = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        let mut reader = BufReader::new(data.as_slice());
        let err = read_message(&mut reader).await.unwrap_err();
        assert!(err.to_string().contains("Content-Length"));
    }

    #[tokio::test]
    async fn write_then_read_round_trip() {
        let body = br#"{"jsonrpc":"2.0","method":"textDocument/didOpen"}"#;
        let mut buf = Vec::new();
        write_message(&mut buf, body).await.unwrap();

        let mut reader = BufReader::new(buf.as_slice());
        let got = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn extra_headers_ignored() {
        // LSP allows Content-Type alongside Content-Length.
        let body = br#"{"id":99}"#;
        let mut frame = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n",
            body.len()
        )
        .into_bytes();
        frame.extend_from_slice(body);

        let mut reader = BufReader::new(frame.as_slice());
        let got = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn zero_length_body() {
        let body = b"";
        let frame = make_frame(body);
        let mut reader = BufReader::new(frame.as_slice());
        let got = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(got, body);
    }
}
