//! Source-time recognition shared by projects, keyed by media content rather than its path.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result, ensure};
use nuzky_engine::{model::Asset, speech::Word};
use serde::{Deserialize, Serialize};

const SAMPLE_BYTES: u64 = 1024 * 1024;
const INTERIOR_CHUNKS: u64 = 32;
const CHUNK_BYTES: u64 = 64 * 1024;
const DURATION_TOLERANCE_US: u64 = 1_000;
use crate::hash::{FNV_OFFSET, hash_bytes};
/// Version 3 records how word times were measured; version 2 records, made before, still read as
/// Whisper's estimates aligned to pauses, so a project and its word corrections keep their words.
pub const VERSION: u32 = 3;
const READABLE: [u32; 2] = [2, VERSION];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_us: i64,
    pub end_us: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    pub fingerprint: String,
    pub duration_us: i64,
    pub model: String,
    pub language: String,
    pub words: Vec<Word>,
    pub segments: Vec<Segment>,
    /// The word timing model that measured the words, None when they are Whisper's estimates
    /// aligned to pauses.
    #[serde(default)]
    pub alignment: Option<String>,
}

struct CachedFingerprint {
    modified: SystemTime,
    size: u64,
    fingerprint: String,
}

#[derive(Clone)]
pub struct TranscriptStore {
    directory: PathBuf,
    events: Option<std::sync::mpsc::Sender<crate::SessionEvent>>,
    fingerprints: Arc<Mutex<HashMap<PathBuf, CachedFingerprint>>>,
}

impl TranscriptStore {
    pub fn open() -> Result<Self> {
        let data = dirs::data_dir().context("STORE_UNAVAILABLE: no data directory")?;
        Self::at(data.join("nuzky/transcripts"))
    }

    pub fn at(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory).context("STORE_UNAVAILABLE: creating transcript directory")?;
        Ok(Self { directory, events: None, fingerprints: Arc::default() })
    }

    pub(crate) fn with_events(mut self, events: Option<std::sync::mpsc::Sender<crate::SessionEvent>>) -> Self {
        self.events = events;
        self
    }

    pub fn fingerprint(&self, asset: &Asset) -> Result<String> {
        let path = Path::new(&asset.path);
        let metadata = fs::metadata(path).context("MEDIA_MISSING: reading transcript source")?;
        let modified = metadata.modified().context("MEDIA_UNREADABLE: modification time")?;
        let size = metadata.len();
        let mut cache = self.fingerprints.lock().unwrap();
        if let Some(hit) = cache.get(path)
            && hit.modified == modified
            && hit.size == size
        {
            return Ok(hit.fingerprint.clone());
        }
        let mut file = File::open(path).context("MEDIA_UNREADABLE: fingerprint source")?;
        let mut hash = FNV_OFFSET;
        hash_bytes(&mut hash, &size.to_le_bytes());
        let mut buffer = vec![0; size.min(SAMPLE_BYTES) as usize];
        for offset in [0, size.saturating_sub(SAMPLE_BYTES)] {
            hash_chunk(&mut file, offset, &mut buffer, &mut hash)?;
        }
        buffer.resize(size.min(CHUNK_BYTES) as usize, 0);
        let span = size.saturating_sub(CHUNK_BYTES);
        for i in 0..INTERIOR_CHUNKS {
            let offset = (u128::from(span) * u128::from(i) / u128::from(INTERIOR_CHUNKS - 1)) as u64;
            hash_chunk(&mut file, offset, &mut buffer, &mut hash)?;
        }
        let fingerprint = format!("{size:016x}-{hash:016x}");
        cache.insert(path.to_owned(), CachedFingerprint { modified, size, fingerprint: fingerprint.clone() });
        Ok(fingerprint)
    }

    pub fn get(&self, asset: &Asset) -> Result<Option<Record>> {
        let fingerprint = self.fingerprint(asset)?;
        let bytes = match fs::read(self.directory.join(format!("{fingerprint}.json"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("STORE_UNREADABLE: reading transcript"),
        };
        #[derive(Deserialize)]
        struct Header {
            version: u32,
        }
        let header: Header = serde_json::from_slice(&bytes).context("INVALID_TRANSCRIPT: parsing version")?;
        if !READABLE.contains(&header.version) {
            return Ok(None);
        }
        let record: Record = serde_json::from_slice(&bytes).context("INVALID_TRANSCRIPT: parsing record")?;
        ensure!(
            READABLE.contains(&record.version) && record.fingerprint == fingerprint,
            "INVALID_TRANSCRIPT: version or fingerprint mismatch"
        );
        if record.duration_us.abs_diff(asset.duration_us) > DURATION_TOLERANCE_US {
            return Ok(None);
        }
        Ok(Some(record))
    }

    pub fn put(&self, asset: &Asset, record: &Record) -> Result<()> {
        let fingerprint = self.fingerprint(asset)?;
        ensure!(
            record.version == VERSION
                && record.fingerprint == fingerprint
                && record.duration_us.abs_diff(asset.duration_us) <= DURATION_TOLERANCE_US,
            "SOURCE_CHANGED: recognition source, duration or record version changed"
        );
        ensure!(
            record.words.iter().all(|w| w.start_us >= 0 && w.end_us >= w.start_us && w.probability.is_finite()),
            "INVALID_TRANSCRIPT: invalid word timing or probability"
        );
        crate::storage::save(&self.directory.join(format!("{fingerprint}.json")), record)
            .context("STORE_WRITE_FAILED: publishing transcript")?;
        if let Some(events) = &self.events {
            let _ = events.send(crate::SessionEvent::TranscriptsChanged);
        }
        Ok(())
    }
}

fn hash_chunk(file: &mut File, offset: u64, buffer: &mut [u8], hash: &mut u64) -> Result<()> {
    file.seek(SeekFrom::Start(offset)).context("MEDIA_UNREADABLE: seeking source")?;
    file.read_exact(buffer).context("MEDIA_UNREADABLE: hashing source")?;
    hash_bytes(hash, buffer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::{edit::new_id, model::AssetKind};

    #[test]
    fn fingerprint_survives_copy_move_and_invalidates_changed_content() {
        let dir = std::env::temp_dir().join(format!("transcripts-{}", new_id()));
        let store = TranscriptStore::at(dir.join("store")).unwrap();
        let path = dir.join("source");
        fs::write(&path, vec![7; 3 * SAMPLE_BYTES as usize]).unwrap();
        let mut asset = Asset {
            id: "a".into(),
            name: "a".into(),
            path: path.to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 1,
            width: 1,
            height: 1,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: None,
        };
        let fingerprint = store.fingerprint(&asset).unwrap();
        let record = Record {
            version: VERSION,
            fingerprint: fingerprint.clone(),
            duration_us: asset.duration_us,
            model: "small".into(),
            language: "cs".into(),
            words: vec![],
            segments: vec![],
            alignment: None,
        };
        store.put(&asset, &record).unwrap();
        let copy = dir.join("copy");
        fs::copy(&path, &copy).unwrap();
        asset.path = copy.to_string_lossy().into();
        assert_eq!(store.get(&asset).unwrap(), Some(record.clone()));
        let moved = dir.join("moved");
        fs::rename(copy, &moved).unwrap();
        asset.path = moved.to_string_lossy().into();
        assert_eq!(store.fingerprint(&asset).unwrap(), fingerprint);
        assert_eq!(store.get(&asset).unwrap(), Some(record.clone()));
        let mut newer = record;
        newer.language = "en".into();
        store.put(&asset, &newer).unwrap();
        assert_eq!(store.get(&asset).unwrap(), Some(newer));
        fs::write(&moved, vec![9; 3 * SAMPLE_BYTES as usize]).unwrap();
        assert_ne!(store.fingerprint(&asset).unwrap(), fingerprint);
        assert!(store.get(&asset).unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn middle_content_changes_fingerprint_and_old_or_wrong_duration_records_are_absent() {
        let dir = std::env::temp_dir().join(format!("transcript-middle-{}", new_id()));
        let store = TranscriptStore::at(dir.join("store")).unwrap();
        let path = dir.join("a");
        let copy = dir.join("b");
        let mut bytes = vec![7; 3 * SAMPLE_BYTES as usize];
        fs::write(&path, &bytes).unwrap();
        bytes[SAMPLE_BYTES as usize..2 * SAMPLE_BYTES as usize].fill(9);
        fs::write(&copy, bytes).unwrap();
        let mut asset = Asset {
            id: "a".into(),
            name: "a".into(),
            path: path.to_string_lossy().into(),
            kind: AssetKind::Video,
            duration_us: 1_000_000,
            width: 1,
            height: 1,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
            mirror: false,
            credit: None,
        };
        let fingerprint = store.fingerprint(&asset).unwrap();
        let record = Record {
            version: VERSION,
            fingerprint: fingerprint.clone(),
            duration_us: asset.duration_us,
            model: "small".into(),
            language: "en".into(),
            words: vec![],
            segments: vec![],
            alignment: None,
        };
        store.put(&asset, &record).unwrap();
        asset.path = copy.to_string_lossy().into();
        assert_ne!(store.fingerprint(&asset).unwrap(), fingerprint);
        assert!(store.get(&asset).unwrap().is_none());
        asset.path = path.to_string_lossy().into();
        asset.duration_us += 1_000;
        assert!(store.get(&asset).unwrap().is_some());
        asset.duration_us += 1;
        assert!(store.get(&asset).unwrap().is_none());
        asset.duration_us = record.duration_us - 1_001;
        assert!(store.get(&asset).unwrap().is_none());
        // A version 2 record, made before words were measured, still reads.
        let mut older = serde_json::to_value(&record).unwrap();
        older["version"] = serde_json::json!(2);
        older.as_object_mut().unwrap().remove("alignment");
        fs::write(store.directory.join(format!("{fingerprint}.json")), serde_json::to_vec(&older).unwrap()).unwrap();
        asset.duration_us = record.duration_us;
        assert_eq!(store.get(&asset).unwrap(), Some(Record { version: 2, ..record.clone() }));
        let mut old = serde_json::to_value(record).unwrap();
        old["version"] = serde_json::json!(1);
        old.as_object_mut().unwrap().remove("duration_us");
        fs::write(store.directory.join(format!("{fingerprint}.json")), serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(store.get(&asset).unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
