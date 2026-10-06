//! Source-time recognition shared by projects, keyed by media content rather than its path.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result, ensure};
use capopen_engine::{model::Asset, speech::Word};
use serde::{Deserialize, Serialize};

const SAMPLE_BYTES: u64 = 1024 * 1024;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
pub const VERSION: u32 = 1;

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
    pub model: String,
    pub language: String,
    pub words: Vec<Word>,
    pub segments: Vec<Segment>,
}

struct CachedFingerprint {
    modified: SystemTime,
    size: u64,
    fingerprint: String,
}

#[derive(Clone)]
pub struct TranscriptStore {
    directory: PathBuf,
    fingerprints: Arc<Mutex<HashMap<PathBuf, CachedFingerprint>>>,
}

impl TranscriptStore {
    pub fn open() -> Result<Self> {
        let data = dirs::data_dir().context("STORE_UNAVAILABLE: no data directory")?;
        Self::at(data.join("capopen/transcripts"))
    }

    pub fn at(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)
            .context("STORE_UNAVAILABLE: creating transcript directory")?;
        Ok(Self {
            directory,
            fingerprints: Arc::default(),
        })
    }

    pub fn fingerprint(&self, asset: &Asset) -> Result<String> {
        let path = Path::new(&asset.path);
        let metadata = fs::metadata(path).context("MEDIA_MISSING: reading transcript source")?;
        let modified = metadata
            .modified()
            .context("MEDIA_UNREADABLE: modification time")?;
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
        let mut buffer = vec![0; size.min(SAMPLE_BYTES) as usize];
        for offset in [0, size.saturating_sub(SAMPLE_BYTES)] {
            file.seek(SeekFrom::Start(offset))
                .context("MEDIA_UNREADABLE: seeking source")?;
            file.read_exact(&mut buffer)
                .context("MEDIA_UNREADABLE: hashing source")?;
            for byte in &buffer {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
            }
        }
        let fingerprint = format!("{size:016x}-{hash:016x}");
        cache.insert(
            path.to_owned(),
            CachedFingerprint {
                modified,
                size,
                fingerprint: fingerprint.clone(),
            },
        );
        Ok(fingerprint)
    }

    pub fn get(&self, asset: &Asset) -> Result<Option<Record>> {
        let fingerprint = self.fingerprint(asset)?;
        let bytes = match fs::read(self.directory.join(format!("{fingerprint}.json"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("STORE_UNREADABLE: reading transcript"),
        };
        let record: Record =
            serde_json::from_slice(&bytes).context("INVALID_TRANSCRIPT: parsing record")?;
        ensure!(
            record.version == VERSION && record.fingerprint == fingerprint,
            "INVALID_TRANSCRIPT: version or fingerprint mismatch"
        );
        Ok(Some(record))
    }

    pub fn put(&self, asset: &Asset, record: &Record) -> Result<()> {
        let fingerprint = self.fingerprint(asset)?;
        ensure!(
            record.version == VERSION && record.fingerprint == fingerprint,
            "SOURCE_CHANGED: recognition source or record version changed"
        );
        ensure!(
            record
                .words
                .iter()
                .all(|w| w.start_us >= 0 && w.end_us >= w.start_us && w.probability.is_finite()),
            "INVALID_TRANSCRIPT: invalid word timing or probability"
        );
        crate::storage::save(&self.directory.join(format!("{fingerprint}.json")), record)
            .context("STORE_WRITE_FAILED: publishing transcript")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capopen_engine::{edit::new_id, model::AssetKind};

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
        };
        let fingerprint = store.fingerprint(&asset).unwrap();
        let record = Record {
            version: VERSION,
            fingerprint: fingerprint.clone(),
            model: "small".into(),
            language: "cs".into(),
            words: vec![],
            segments: vec![],
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
}
