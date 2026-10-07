//! `capopen style`: learn a creator's EDIT.md from recordings and their finished cuts, and score
//! any cut of a recording against the creator's own. Everything runs on this computer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail, ensure};
use capopen_analysis::style::{self, Alignment};
use capopen_engine::model::{Asset, AssetKind};
use capopen_engine::speech::map_words;
use capopen_mcp::transcript::{best_model, models, recognise};
use capopen_session::transcripts::{Record, TranscriptStore};
use serde_json::json;

pub const USAGE: &str =
    "  capopen style learn [--out <EDIT.md>] [--replace] [--lang auto|cs|en] <recording> <cut> [<recording> <cut>...]
  capopen style compare [--lang auto|cs|en] <recording> <cut> <project.capopen>";

/// A media file with its recognised words, stored like the app stores them.
struct Recording {
    asset: Asset,
    record: Record,
}

pub fn run(args: &[String], cache: &Path) -> Result<()> {
    let mut language = "auto".to_owned();
    let mut out = None;
    let mut replace = false;
    let mut files = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--lang" => language = rest.next().context("--lang needs a value")?.clone(),
            "--out" => out = Some(PathBuf::from(rest.next().context("--out needs a path")?)),
            "--replace" => replace = true,
            _ if arg.starts_with("--") => bail!("Unknown option {arg}\n{USAGE}"),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let store = TranscriptStore::open()?;
    match args.first().map(String::as_str) {
        Some("compare") if files.len() == 3 && out.is_none() && !replace => {
            let recording = recording(&files[0], &language, &store, cache)?;
            let cut = self::recording(&files[1], &language, &store, cache)?;
            let alignment = aligned(&recording, &cut.asset, cache)?;
            let human: Vec<bool> =
                alignment.places(&recording.record.words, &cut.record.words).iter().map(Option::is_some).collect();
            let agent = project_keeps(&files[2], &recording, &store)?;
            let score = style::score(&recording.record.words, &human, &agent);
            println!("{}", serde_json::to_string_pretty(&json!({"score": score, "pieces": alignment.pieces}))?);
        }
        Some("learn") if !files.is_empty() && files.len() % 2 == 0 => {
            let out = out.unwrap_or_else(style::style_path);
            ensure!(
                replace || out.symlink_metadata().is_err(),
                "{} already exists and may hold your own changes. Pass --replace to overwrite it, or --out for another file.",
                out.display()
            );
            let mut learned = Vec::new();
            for pair in files.chunks(2) {
                let recording = self::recording(&pair[0], &language, &store, cache)?;
                let cut = self::recording(&pair[1], &language, &store, cache)?;
                eprintln!("Comparing {} with {}", pair[1].display(), pair[0].display());
                let alignment = aligned(&recording, &cut.asset, cache)?;
                let picture = style::picture(&recording.asset, &cut.asset, &alignment, &|| false)?;
                learned.push((recording, cut, alignment, picture));
            }
            let sources: Vec<style::Source> = learned
                .iter()
                .map(|(recording, cut, alignment, picture)| style::Source {
                    recording: recording.asset.name.clone(),
                    cut: cut.asset.name.clone(),
                    language: recording.record.language.clone(),
                    recording_us: recording.asset.duration_us,
                    words: &recording.record.words,
                    cut_words: &cut.record.words,
                    alignment,
                    picture,
                })
                .collect();
            write_new(&out, style::learn(&sources).as_bytes())?;
            eprintln!("Wrote {}", out.display());
        }
        _ => bail!("Usage:\n{USAGE}"),
    }
    Ok(())
}

fn probe(path: &Path) -> Result<Asset> {
    let asset = capopen_engine::media::probe(path, stable_id(path)?)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(capopen_engine::audio::has_audio(&asset), "{} has no sound", path.display());
    Ok(asset)
}

/// The same file gets the same id, so its sound is extracted once into the shared cache.
fn stable_id(path: &Path) -> Result<String> {
    use std::hash::{Hash, Hasher};
    let path = std::fs::canonicalize(path).with_context(|| format!("Cannot find {}", path.display()))?;
    let metadata = path.metadata()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (&path, metadata.len(), metadata.modified()?).hash(&mut hash);
    Ok(format!("style-{:016x}", hash.finish()))
}

/// Words stored for this file, recognised now if it has none yet.
fn recording(path: &Path, language: &str, store: &TranscriptStore, cache: &Path) -> Result<Recording> {
    let asset = probe(path)?;
    // CapOpen hears speech in videos only; a sound file on the timeline is music.
    ensure!(
        asset.kind == AssetKind::Video,
        "{} has no picture: style learns from and scores video recordings",
        path.display()
    );
    // A language asked for explicitly corrects a recognition stored in another one.
    if let Some(record) = store.get(&asset)?
        && (language == "auto" || record.language == language)
    {
        return Ok(Recording { asset, record });
    }
    let model = best_model();
    let paths = models(model)?;
    eprintln!("Recognising speech in {} with {model}", path.display());
    let record = recognise(store, &asset, cache, model, &paths, language, &AtomicBool::new(false), |_| {})?;
    Ok(Recording { asset, record })
}

fn aligned(recording: &Recording, cut: &Asset, cache: &Path) -> Result<Alignment> {
    let alignment = style::align(&recording.asset, cut, cache, &|| false)?;
    ensure!(
        alignment.matched >= 0.6,
        "{} does not look cut from {}: only {:.0}% of its speech was found in the recording",
        cut.path,
        recording.asset.path,
        alignment.matched * 100.0
    );
    Ok(alignment)
}

/// Which recording words a project plays: a word counts when the middle of it is heard.
fn project_keeps(path: &Path, recording: &Recording, store: &TranscriptStore) -> Result<Vec<bool>> {
    let project = crate::load(path.to_str().context("Project path must be UTF-8")?)?;
    let fingerprint = store.fingerprint(&recording.asset)?;
    let transcripts: HashMap<String, Vec<_>> = project
        .assets
        .iter()
        .filter(|a| store.fingerprint(a).is_ok_and(|f| f == fingerprint))
        .map(|a| (a.id.clone(), recording.record.words.clone()))
        .collect();
    ensure!(!transcripts.is_empty(), "{} does not use {}", path.display(), recording.asset.path);
    let heard: HashSet<i64> = map_words(&project, &transcripts).iter().map(|w| w.source_start_us).collect();
    Ok(recording.record.words.iter().map(|w| heard.contains(&w.start_us)).collect())
}

/// Writes beside the target first, so a failure never leaves half a file.
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let tmp = path
        .with_file_name(format!(".{}.part", path.file_name().context("Output needs a file name")?.to_string_lossy()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .with_context(|| format!("Cannot write {}", path.display()))
}
