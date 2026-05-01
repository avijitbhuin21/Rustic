//! NDJSON framing helpers for the harness ↔ CLI transport.
//!
//! Both Claude Code (`--input-format stream-json --output-format stream-json`)
//! and Codex (`app-server`, JSON-RPC over stdio) speak newline-delimited JSON.
//! This module exposes thin readers/writers that:
//!
//! * Read one envelope per line from a `tokio` async reader, yielding parsed
//!   `serde_json::Value`s through a `Stream`. We parse to `Value` here on
//!   purpose — typed translation lives in `event_map.rs` so that schema drift
//!   in a single field doesn't sink the entire stream.
//! * Write one envelope per line to a `tokio` async writer, with a `\n`
//!   terminator (never `\r\n`, even on Windows — plan §8.1).
//!
//! Both halves are split-friendly: the reader takes any `AsyncRead` and the
//! writer takes any `AsyncWrite`, so harness implementations can pair them
//! with a child's stdout/stdin without coupling.

use anyhow::{Context, Result};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::sync::Mutex;

/// Async stream of parsed JSON envelopes from a CLI's stdout.
///
/// Lines that fail to parse are surfaced as an `Err` item — the consumer
/// should log and continue, not abort the session, since one malformed line
/// from a CLI shouldn't kill an in-progress conversation.
pub struct NdjsonReader<R: AsyncRead + Unpin> {
    lines: Lines<BufReader<R>>,
}

impl<R: AsyncRead + Unpin> NdjsonReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            lines: BufReader::new(reader).lines(),
        }
    }

    /// Read the next envelope. Returns `Ok(None)` on EOF (CLI exited cleanly).
    pub async fn next_envelope(&mut self) -> Result<Option<serde_json::Value>> {
        loop {
            match self.lines.next_line().await.context("reading NDJSON line")? {
                None => return Ok(None),
                Some(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let v = serde_json::from_str::<serde_json::Value>(trimmed)
                        .with_context(|| format!("malformed NDJSON envelope: {trimmed}"))?;
                    return Ok(Some(v));
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin> Stream for NdjsonReader<R> {
    type Item = Result<serde_json::Value>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        // Project the inner Lines<...> and poll it.
        let this = self.get_mut();
        loop {
            match Pin::new(&mut this.lines).poll_next_line(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(None)) => return Poll::Ready(None),
                Poll::Ready(Ok(Some(line))) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let parsed = serde_json::from_str::<serde_json::Value>(trimmed)
                        .with_context(|| format!("malformed NDJSON envelope: {trimmed}"));
                    return Poll::Ready(Some(parsed));
                }
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("read line error: {e}"))));
                }
            }
        }
    }
}

/// Writer side. Wraps an `AsyncWrite` behind a `Mutex` so multiple senders
/// (the user-message path, the permission-response path, the interrupt path)
/// can serialise envelopes without tearing each other.
pub struct NdjsonWriter<W: AsyncWrite + Unpin + Send> {
    inner: Mutex<W>,
}

impl<W: AsyncWrite + Unpin + Send> NdjsonWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            inner: Mutex::new(writer),
        }
    }

    /// Serialise `envelope` as compact JSON, append a single `\n`, and flush.
    /// We always flush per envelope — these are low-frequency control messages
    /// and unflushed envelopes would just stall the CLI waiting for input.
    pub async fn write(&self, envelope: &serde_json::Value) -> Result<()> {
        let mut buf = serde_json::to_vec(envelope).context("serialising NDJSON envelope")?;
        buf.push(b'\n');
        let mut g = self.inner.lock().await;
        g.write_all(&buf).await.context("writing NDJSON envelope")?;
        g.flush().await.context("flushing NDJSON envelope")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trip_two_envelopes() {
        let (a, b) = duplex(1024);
        let writer = NdjsonWriter::new(a);
        writer
            .write(&serde_json::json!({"type": "ping", "n": 1}))
            .await
            .unwrap();
        writer
            .write(&serde_json::json!({"type": "ping", "n": 2}))
            .await
            .unwrap();
        // Drop the writer side so the reader sees EOF after two lines.
        drop(writer);

        let mut reader = NdjsonReader::new(b);
        let first = reader.next_envelope().await.unwrap().unwrap();
        assert_eq!(first["n"], 1);
        let second = reader.next_envelope().await.unwrap().unwrap();
        assert_eq!(second["n"], 2);
        assert!(reader.next_envelope().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn skips_blank_lines() {
        let (mut a, b) = duplex(1024);
        a.write_all(b"\n{\"x\":1}\n\n{\"x\":2}\n").await.unwrap();
        drop(a);

        let mut reader = NdjsonReader::new(b);
        assert_eq!(reader.next_envelope().await.unwrap().unwrap()["x"], 1);
        assert_eq!(reader.next_envelope().await.unwrap().unwrap()["x"], 2);
        assert!(reader.next_envelope().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_line_surfaces_error() {
        let (mut a, b) = duplex(1024);
        a.write_all(b"not json\n").await.unwrap();
        drop(a);

        let mut reader = NdjsonReader::new(b);
        assert!(reader.next_envelope().await.is_err());
    }
}
