//! Minimal Server-Sent Events (SSE) line parser for streaming LLM responses.
//!
//! Parses the raw byte stream from an SSE HTTP response into typed `SseLine`
//! values.  Consumers feed these into a provider-specific `parse_stream_event`
//! implementation to produce `StreamChunk`s.
//!
//! SSE wire format (RFC 8895 subset used by LLM providers):
//! ```text
//! event: <event-type>\n
//! data: <json-payload>\n
//! \n
//! ```
//! Lines starting with `:` are comments/keepalives and are skipped.
//! The sentinel `data: [DONE]` signals end-of-stream for OpenAI-compatible APIs.

use bytes::Bytes;
use futures::stream::{Stream, StreamExt};

/// A parsed SSE line emitted by the byte-stream decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseLine {
    /// `event: <name>` line.
    Event(String),
    /// `data: <payload>` line (non-empty, non-`[DONE]`).
    Data(String),
    /// The OpenAI-convention end-of-stream sentinel `data: [DONE]`.
    Done,
}

/// Decode an async byte stream (e.g. from `reqwest::Response::bytes_stream()`)
/// into a stream of `SseLine`s.
///
/// This function buffers incomplete lines across chunk boundaries so callers
/// never receive a partial line.
pub fn sse_lines(
    byte_stream: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
) -> impl Stream<Item = anyhow::Result<SseLine>> + Send + 'static {
    let mut buf = String::new();

    byte_stream.flat_map(move |chunk| {
        let lines_out: Vec<anyhow::Result<SseLine>> = match chunk {
            Err(e) => vec![Err(anyhow::anyhow!("SSE read error: {e}"))],
            Ok(bytes) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                let mut results = Vec::new();

                // Process all complete lines (terminated by \n).
                while let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim_end_matches('\r').to_string();
                    buf = buf[nl + 1..].to_string();
                    if let Some(parsed) = parse_line(&line) {
                        results.push(Ok(parsed));
                    }
                }
                results
            }
        };
        futures::stream::iter(lines_out)
    })
}

/// Parse a single SSE text line.  Returns `None` for blank lines and comments.
fn parse_line(line: &str) -> Option<SseLine> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("data: ") {
        if rest == "[DONE]" {
            return Some(SseLine::Done);
        }
        if !rest.is_empty() {
            return Some(SseLine::Data(rest.to_string()));
        }
        return None;
    }
    if let Some(rest) = line.strip_prefix("event: ") {
        return Some(SseLine::Event(rest.to_string()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_data() {
        assert_eq!(
            parse_line(r#"data: {"type":"text"}"#),
            Some(SseLine::Data(r#"{"type":"text"}"#.into()))
        );
    }

    #[test]
    fn parse_line_event() {
        assert_eq!(
            parse_line("event: content_block_delta"),
            Some(SseLine::Event("content_block_delta".into()))
        );
    }

    #[test]
    fn parse_line_done() {
        assert_eq!(parse_line("data: [DONE]"), Some(SseLine::Done));
    }

    #[test]
    fn parse_line_blank_and_comment_returns_none() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line(": keepalive"), None);
    }

    #[test]
    fn parse_line_crlf_stripped() {
        assert_eq!(
            parse_line("data: hello\r"),
            Some(SseLine::Data("hello".into()))
        );
    }

    #[tokio::test]
    async fn sse_lines_yields_correct_sequence() {
        use bytes::Bytes;
        use futures::stream;

        let chunks: Vec<reqwest::Result<Bytes>> = vec![
            Ok(Bytes::from("event: message_start\ndata: {\"t\":1}\n\n")),
            Ok(Bytes::from("data: [DONE]\n")),
        ];
        let stream = stream::iter(chunks);
        let lines: Vec<_> = sse_lines(stream).collect().await;

        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0].as_ref().unwrap(),
            &SseLine::Event("message_start".into())
        );
        assert_eq!(
            lines[1].as_ref().unwrap(),
            &SseLine::Data("{\"t\":1}".into())
        );
        assert_eq!(lines[2].as_ref().unwrap(), &SseLine::Done);
    }

    #[tokio::test]
    async fn sse_lines_handles_line_split_across_chunks() {
        use bytes::Bytes;
        use futures::stream;

        // Line split: "dat" in first chunk, "a: hello\n" in second.
        let chunks: Vec<reqwest::Result<Bytes>> =
            vec![Ok(Bytes::from("dat")), Ok(Bytes::from("a: hello\n"))];
        let stream = stream::iter(chunks);
        let lines: Vec<_> = sse_lines(stream).collect().await;

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].as_ref().unwrap(), &SseLine::Data("hello".into()));
    }
}
