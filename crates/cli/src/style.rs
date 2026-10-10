//! `nuzky style`: learn a creator's EDIT.md from recordings and their finished cuts, and score
//! any cut of a recording against the creator's own. Everything runs on this computer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail, ensure};
use nuzky_analysis::style as learning;
use nuzky_engine::speech::map_words;
use nuzky_mcp::style;
use nuzky_mcp::transcript::best_model;
use nuzky_session::transcripts::TranscriptStore;
use serde_json::json;

pub const USAGE: &str =
    "  nuzky style learn [--out <EDIT.md>] [--replace] [--lang auto|cs|en] <recording> <cut> [<recording> <cut>...]
  nuzky style compare [--lang auto|cs|en] <recording> <cut> <project.nuzky>";

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
    let video = |path: &Path| {
        let mut told = false;
        style::video(path, &language, &store, cache, &AtomicBool::new(false), |_| {
            if !std::mem::replace(&mut told, true) {
                eprintln!("Recognising speech in {} with {}", path.display(), best_model());
            }
        })
    };
    match args.first().map(String::as_str) {
        Some("compare") if files.len() == 3 && out.is_none() && !replace => {
            let recording = video(&files[0])?;
            let cut = video(&files[1])?;
            let alignment = style::aligned(&recording, &cut.asset, cache, &|| false)?;
            let human: Vec<bool> =
                alignment.places(&recording.record.words, &cut.record.words).iter().map(Option::is_some).collect();
            let agent = project_keeps(&files[2], &recording, &store)?;
            let score = learning::score(&recording.record.words, &human, &agent);
            println!("{}", serde_json::to_string_pretty(&json!({"score": score, "pieces": alignment.pieces}))?);
        }
        Some("learn") if !files.is_empty() && files.len() % 2 == 0 => {
            let default = learning::style_path();
            let out = out.unwrap_or_else(|| default.clone());
            ensure!(
                replace || out.symlink_metadata().is_err(),
                "{} already exists and may hold your own changes. Pass --replace to overwrite it, or --out for another file.",
                out.display()
            );
            let mut learned = Vec::new();
            for pair in files.chunks(2) {
                let recording = video(&pair[0])?;
                let cut = video(&pair[1])?;
                eprintln!("Comparing {} with {}", pair[1].display(), pair[0].display());
                learned.push(style::compare(&recording, &cut, &store, cache, &|| false)?);
            }
            let sources: Vec<learning::Source> = learned.iter().map(style::Evidence::source).collect();
            let learned = learning::learned(&sources);
            // The style the app shows is a version, keeps the creator's own rules and can be undone.
            if same_file(&out, &default) {
                style::Store::default().replace(&learned)?;
            } else {
                write_new(&out, learned.document().as_bytes())?;
            }
            eprintln!("Wrote {}", out.display());
        }
        _ => bail!("Usage:\n{USAGE}"),
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| {
        let parent = p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::canonicalize(parent).ok().map(|dir| dir.join(p.file_name().unwrap_or_default()))
    };
    a == b || canonical(a).is_some_and(|a| canonical(b).is_some_and(|b| a == b))
}

/// Which recording words a project plays: a word counts when the middle of it is heard.
fn project_keeps(path: &Path, recording: &style::Video, store: &TranscriptStore) -> Result<Vec<bool>> {
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
