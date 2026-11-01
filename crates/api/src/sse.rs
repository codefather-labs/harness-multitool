//! Server-sent events, for providers that stream over HTTP. Bytes go in as they arrive; each
//! finished event's `data` comes out.

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    data: String,
}

impl SseParser {
    /// Feeds a chunk, returning the `data` of every event it completed.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(std::mem::take(&mut self.data));
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_anywhere_come_out_whole() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: a\ndata: {\"x\"").is_empty());
        assert_eq!(p.feed(b":1}\r\n\r\ndata: two\n\n"), vec!["{\"x\":1}", "two"]);
        assert_eq!(p.feed("data: привет\n\n".as_bytes()), vec!["привет"]);
    }
}
