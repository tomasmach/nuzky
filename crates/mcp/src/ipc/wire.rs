use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use serde::{
    Serialize,
    de::{DeserializeOwned, IgnoredAny, MapAccess, Visitor},
};

use crate::limits::MAX_LINE_BYTES;

pub(super) fn send(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    stream.write_all(&encode(value)?).context("IPC_CLOSED: writing message")
}

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).context("PROTOCOL_MISMATCH: serializing message")?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= MAX_LINE_BYTES, "RESULT_TOO_LARGE: IPC message exceeds transport limit");
    Ok(bytes)
}

pub(super) struct Line {
    pub bytes: Vec<u8>,
    pub complete: bool,
}

pub(super) fn line(reader: &mut BufReader<UnixStream>, limit: usize, deadline: Option<Instant>) -> Result<Line> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        if let Some(deadline) = deadline {
            let remaining =
                deadline.checked_duration_since(Instant::now()).context("IPC_UNTRUSTED: hello deadline exceeded")?;
            reader.get_ref().set_read_timeout(Some(remaining))?;
        }
        let buffered = reader.fill_buf().context("IPC_UNTRUSTED: reading IPC message")?;
        ensure!(!buffered.is_empty(), "IPC_CLOSED: peer disconnected");
        let available = buffered.len().min(limit - bytes.len());
        let end = buffered[..available].iter().position(|byte| *byte == b'\n');
        let length = end.map_or(available, |end| end + 1);
        bytes.extend_from_slice(&buffered[..length]);
        reader.consume(length);
        if end.is_some() {
            return Ok(Line { bytes, complete: true });
        }
    }
    Ok(Line { bytes, complete: false })
}

pub(super) fn receive<T: DeserializeOwned>(
    reader: &mut BufReader<UnixStream>,
    limit: usize,
    deadline: Option<Instant>,
) -> Result<T> {
    let line = line(reader, limit, deadline)?;
    ensure!(line.complete, "PROTOCOL_MISMATCH: message exceeds line limit");
    serde_json::from_slice(&line.bytes).context("PROTOCOL_MISMATCH: invalid message")
}

/// An oversized JSON prefix can still contain its id before the truncated args.
pub(super) fn request_id(bytes: &[u8]) -> Option<u64> {
    struct Id<'a>(&'a std::cell::Cell<Option<u64>>);
    impl<'de> Visitor<'de> for Id<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("request id")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if key == "id" {
                    self.0.set(Some(map.next_value()?));
                    return Ok(());
                }
                map.next_value::<IgnoredAny>()?;
            }
            Ok(())
        }
    }
    let found = std::cell::Cell::new(None);
    // deserialize_map also checks the closing brace; the truncated tail is deliberately ignored.
    let _ = serde::Deserializer::deserialize_map(&mut serde_json::Deserializer::from_slice(bytes), Id(&found));
    found.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn hello_deadline_cannot_be_extended_by_dripping_bytes() {
        let (reader, mut sender) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..20 {
                if sender.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        });
        let began = Instant::now();
        assert!(line(&mut BufReader::new(reader), 4096, Some(began + Duration::from_millis(80))).is_err());
        assert!(began.elapsed() < Duration::from_millis(250));
        writer.join().unwrap();
    }

    #[test]
    fn oversized_line_retains_request_id_without_allocating_the_rest() {
        let (reader, mut sender) = UnixStream::pair().unwrap();
        sender.write_all(br#"{"id":37,"tool":"x","args":{"payload":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"#).unwrap();
        let line = line(&mut BufReader::new(reader), 64, None).unwrap();
        assert!(!line.complete);
        assert_eq!(line.bytes.len(), 64);
        assert_eq!(request_id(&line.bytes), Some(37));
    }
}
