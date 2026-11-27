//! Incremental `text/event-stream` decoder used by the Streamable HTTP
//! transport.

use crate::error::{Error, Result};

/// Largest event the decoder buffers, matching the stdio line limit.
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

/// Turns arbitrary byte chunks into the `data` payloads of complete events.
#[derive(Default)]
pub(crate) struct SseDecoder {
    line: Vec<u8>,
    data: String,
}

impl SseDecoder {
    /// Feed a chunk; returns the data of every event completed by it.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>> {
        let mut events = Vec::new();
        for &byte in chunk {
            if byte != b'\n' {
                self.line.push(byte);
                if self.line.len() + self.data.len() > MAX_EVENT_BYTES {
                    return Err(Error::message("MCP SSE event exceeded transport limit"));
                }
                continue;
            }
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8(line)
                .map_err(|_| Error::message("MCP SSE stream is not valid UTF-8"))?;
            if let Some(event) = self.take_line(&line) {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Flush an event left open when the stream ended without a blank line.
    pub(crate) fn finish(&mut self) -> Option<String> {
        if !self.line.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
            if let Some(event) = self.take_line(&line) {
                return Some(event);
            }
        }
        (!self.data.is_empty()).then(|| std::mem::take(&mut self.data))
    }

    fn take_line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            return (!self.data.is_empty()).then(|| std::mem::take(&mut self.data));
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `event:`, `id:`, `retry:` and `:` comments carry nothing we use.
        None
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_events_split_across_chunks() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"event: message\r\nda").unwrap().is_empty());
        assert!(decoder.push(b"ta: {\"a\":").unwrap().is_empty());
        let events = decoder.push(b"1}\r\n\r\n: keepalive\n\ndata: x\n").unwrap();
        assert_eq!(events, vec!["{\"a\":1}".to_string()]);
        assert_eq!(decoder.finish(), Some("x".into()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn joins_multi_line_data_and_flushes_unterminated_line() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"data: a\ndata:b\n\n").unwrap();
        assert_eq!(events, vec!["a\nb".to_string()]);
        decoder.push(b"data: tail").unwrap();
        assert_eq!(decoder.finish(), Some("tail".into()));
    }

    #[test]
    fn rejects_invalid_utf8_and_oversized_events() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: \xff\n").is_err());
        let mut decoder = SseDecoder::default();
        let huge = vec![b'a'; MAX_EVENT_BYTES + 1];
        assert!(decoder.push(&huge).is_err());
    }
}
