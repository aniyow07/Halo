//! Incremental Server-Sent Events parser.
//!
//! Network chunks can split anywhere: inside a JSON object, inside a line
//! terminator, even inside a multi-byte UTF-8 character. The parser buffers
//! raw bytes and only decodes complete lines, so none of those splits can
//! corrupt an event.

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns the `data` payloads of every event completed by
    /// this chunk, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.handle_line(&line, &mut events);
        }
        events
    }

    /// Flush at end of stream. A final event that was not terminated by a
    /// blank line is still delivered (some servers omit the trailing blank
    /// line before closing).
    pub fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let mut line = std::mem::take(&mut self.buffer);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.handle_line(&line, &mut events);
        }
        self.dispatch(&mut events);
        events
    }

    /// True when bytes or event lines are pending (i.e. the stream ended
    /// mid-event).
    pub fn has_pending(&self) -> bool {
        !self.buffer.iter().all(|b| b.is_ascii_whitespace()) || !self.data_lines.is_empty()
    }

    fn handle_line(&mut self, line: &[u8], events: &mut Vec<String>) {
        if line.is_empty() {
            self.dispatch(events);
            return;
        }
        if line[0] == b':' {
            return; // comment / keep-alive
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &[][..]),
        };
        if field == b"data" {
            self.data_lines
                .push(String::from_utf8_lossy(value).into_owned());
        }
        // `event`, `id`, `retry` are not used by the Gemini stream.
    }

    fn dispatch(&mut self, events: &mut Vec<String>) {
        if self.data_lines.is_empty() {
            return;
        }
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        if !data.trim().is_empty() {
            events.push(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&[u8]]) -> Vec<String> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push(c));
        }
        out.extend(p.finish());
        out
    }

    #[test]
    fn parses_complete_events() {
        let out = feed_all(&[b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn handles_arbitrary_chunk_boundaries() {
        let stream = b"data: {\"text\":\"hello\"}\r\n\r\ndata: {\"text\":\"world\"}\r\n\r\n";
        for split in 1..stream.len() {
            let out = feed_all(&[&stream[..split], &stream[split..]]);
            assert_eq!(
                out,
                vec!["{\"text\":\"hello\"}", "{\"text\":\"world\"}"],
                "split at {split}"
            );
        }
        // One byte at a time.
        let chunks: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(feed_all(&chunks).len(), 2);
    }

    #[test]
    fn keeps_multibyte_utf8_split_across_chunks_intact() {
        let stream = "data: {\"text\":\"héllo 世界 🚀\"}\n\n".as_bytes();
        for split in 1..stream.len() {
            let out = feed_all(&[&stream[..split], &stream[split..]]);
            assert_eq!(out, vec!["{\"text\":\"héllo 世界 🚀\"}"]);
        }
    }

    #[test]
    fn joins_multi_line_data_and_ignores_comments_and_other_fields() {
        let out = feed_all(&[b": keep-alive\nevent: message\nid: 4\ndata: line1\ndata: line2\n\n"]);
        assert_eq!(out, vec!["line1\nline2"]);
    }

    #[test]
    fn skips_empty_events() {
        let out = feed_all(&[b"\n\ndata:\n\ndata: \n\n\n"]);
        assert!(out.is_empty());
    }

    #[test]
    fn flushes_unterminated_final_event() {
        let out = feed_all(&[b"data: {\"last\":true}"]);
        assert_eq!(out, vec!["{\"last\":true}"]);
    }

    #[test]
    fn reports_pending_partial_data() {
        let mut p = SseParser::new();
        p.push(b"data: {\"partial\":");
        assert!(p.has_pending());
        let mut p = SseParser::new();
        p.push(b"data: {}\n\n");
        assert!(!p.has_pending());
    }
}
