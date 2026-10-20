//! Minimal server-sent-events decoder over a byte stream.

use futures::{Stream, StreamExt};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    /// Feed a chunk of bytes, returning any complete events.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() || self.event.is_some() {
                    out.push(SseEvent {
                        event: self.event.take(),
                        data: std::mem::take(&mut self.data).join("\n"),
                    });
                }
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = Some(v.trim().to_string());
            }
        }
        out
    }
}

pub fn events(
    resp: reqwest::Response,
) -> impl Stream<Item = anyhow::Result<SseEvent>> + Send + Unpin {
    let mut decoder = SseDecoder::default();
    Box::pin(
        resp.bytes_stream()
            .map(move |chunk| -> anyhow::Result<Vec<SseEvent>> {
                let chunk = chunk?;
                Ok(decoder.push(&chunk))
            })
            .flat_map(|r| {
                futures::stream::iter(match r {
                    Ok(v) => v.into_iter().map(Ok).collect::<Vec<_>>(),
                    Err(e) => vec![Err(e)],
                })
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_split_events() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"event: ping\nda").is_empty());
        let ev = d.push(b"ta: {\"a\":1}\n\ndata: x\n\n");
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].event.as_deref(), Some("ping"));
        assert_eq!(ev[0].data, "{\"a\":1}");
        assert_eq!(ev[1].data, "x");
    }
}
