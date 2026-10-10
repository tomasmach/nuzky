//! The Audio panel's sound library: search, preview, add at the playhead, credits and the Freesound
//! setting. Searching and downloading live in `nuzky_mcp::sounds`, shared with agents.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nuzky_engine::Project;
use nuzky_engine::edit::EditCmd;
use nuzky_engine::model::AssetKind;
use nuzky_mcp::sounds::{self, Kind, Page, Sound};
use nuzky_session::Expect;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::audio_out::AudioOut;
use crate::{AppState, CmdResult, Snapshot, err, lock_session};

/// The preview playing, and the downloads running, by sound id: setting a flag stops it.
#[derive(Default)]
pub struct Sounds {
    preview: Mutex<Option<Arc<AtomicBool>>>,
    downloads: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    id: &'a str,
    progress: f32,
}

fn progress<'a>(app: &'a AppHandle, id: &'a str) -> impl FnMut(f32) + 'a {
    let mut last = -1.0f32;
    move |p| {
        if p - last >= 0.02 || p >= 1.0 {
            last = p;
            app.emit("sound-progress", Progress { id, progress: p }).ok();
        }
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(err)?.map_err(err)
}

/// The sound effects built into Nuzky.
#[tauri::command]
pub fn sound_library() -> Vec<Sound> {
    sounds::built_in("")
}

#[tauri::command]
pub async fn sound_search(query: String, kind: Kind, page: u32) -> CmdResult<Page> {
    blocking(move || sounds::search(&query, kind, None, page)).await
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStarted {
    duration_us: i64,
}

/// Plays a sound through the timeline's own mixer, downloading it first when needed. A new preview,
/// `sound_stop` or the end of the sound stops it; at the end `sound-preview-ended` names it.
#[tauri::command]
pub async fn sound_preview(app: AppHandle, id: String) -> CmdResult<PreviewStarted> {
    let state = app.state::<AppState>();
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = state.sounds.preview.lock().unwrap().replace(stop.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    let cache = state.cache_dir.clone();
    let (app2, id2, stop2) = (app.clone(), id.clone(), stop.clone());
    let (project, duration_us) = blocking(move || {
        let (path, _) = sounds::fetch(&id2, &cache, || stop2.load(Ordering::Relaxed), progress(&app2, &id2))?;
        let asset = nuzky_engine::media::probe(&path, "sound-preview".into())?;
        anyhow::ensure!(asset.kind == AssetKind::Audio, "SOUND_UNAVAILABLE: this file is not a sound");
        nuzky_engine::audio::ensure_pcm(&cache, &asset, |_| nuzky_session::jobs::check_cancel(&stop2))?;
        let mut project = Project::new("Preview");
        let duration_us = asset.duration_us;
        project.apply(EditCmd::AddAssets { assets: vec![asset] })?;
        project.apply(EditCmd::AddClip { asset_id: "sound-preview".into(), start_us: Some(0), track_id: None })?;
        Ok((project, duration_us))
    })
    .await?;
    if stop.load(Ordering::Relaxed) {
        return Err("CANCELLED: another sound is playing".into());
    }
    let cache = state.cache_dir.clone();
    // The output stream stays on the thread that opened it.
    std::thread::Builder::new()
        .name("sound-preview".into())
        .spawn(move || {
            let out = AudioOut::start(Arc::new(Mutex::new(Arc::new(project))), 0, cache);
            while !stop.load(Ordering::Relaxed) && out.now_us() < duration_us + 150_000 {
                std::thread::sleep(Duration::from_millis(30));
            }
            drop(out);
            if !stop.load(Ordering::Relaxed) {
                app.emit("sound-preview-ended", &id).ok();
            }
        })
        .map_err(err)?;
    Ok(PreviewStarted { duration_us })
}

#[tauri::command]
pub fn sound_stop(state: State<'_, AppState>) {
    if let Some(stop) = state.sounds.preview.lock().unwrap().take() {
        stop.store(true, Ordering::Relaxed);
    }
}

/// Stops downloading a sound being added.
#[tauri::command]
pub fn sound_cancel(state: State<'_, AppState>, id: String) {
    if let Some(cancel) = state.sounds.downloads.lock().unwrap().get(&id) {
        cancel.store(true, Ordering::Relaxed);
    }
}

/// Adds the sound at `start_us` on an audio track as one undo step, downloading it first when needed.
/// A sound the project already has gets another clip of the same asset.
#[tauri::command]
pub async fn sound_add(
    app: AppHandle,
    id: String,
    start_us: i64,
    expected_epoch: Option<String>,
) -> CmdResult<Snapshot> {
    let state = app.state::<AppState>();
    let existing = sounds::existing(&state.project()?.assets, &id).map(|a| a.id.clone());
    let asset = match existing {
        Some(_) => None,
        None => {
            let cancel = Arc::new(AtomicBool::new(false));
            if state.sounds.downloads.lock().unwrap().insert(id.clone(), cancel.clone()).is_some() {
                return Err("BUSY: this sound is already being added".into());
            }
            let (cache, app2, id2) = (state.cache_dir.clone(), app.clone(), id.clone());
            let result =
                blocking(move || sounds::asset(&id2, &cache, || cancel.load(Ordering::Relaxed), progress(&app2, &id2)))
                    .await;
            state.sounds.downloads.lock().unwrap().remove(&id);
            Some(result?)
        }
    };
    let asset_id = existing.or_else(|| asset.as_ref().map(|a| a.id.clone())).unwrap_or_default();
    let mut cmds: Vec<EditCmd> = asset.into_iter().map(|asset| EditCmd::AddAssets { assets: vec![asset] }).collect();
    cmds.push(EditCmd::AddClip { asset_id, start_us: Some(start_us.max(0)), track_id: None });
    let snap = state.apply_batch(cmds, None, Expect::default(), expected_epoch.as_deref())?;
    crate::jobs::prepare_media(&state, &snap.project);
    Ok(snap)
}

/// The credits the video's description needs, as export writes them beside it; none without CC BY sounds.
#[tauri::command]
pub fn sound_credits(state: State<'_, AppState>, expected_epoch: Option<String>) -> CmdResult<Option<String>> {
    let project = lock_session(&state.session, expected_epoch.as_deref())?.host.session.state().map_err(err)?.project;
    Ok(nuzky_engine::credits::credits(&project))
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundSettings {
    freesound: bool,
    has_key: bool,
}

#[tauri::command]
pub fn sound_settings() -> SoundSettings {
    let settings = sounds::settings();
    SoundSettings { freesound: settings.freesound, has_key: !settings.freesound_key.is_empty() }
}

/// `key` replaces the stored Freesound key; None keeps it, empty removes it. The key never comes back.
#[tauri::command]
pub fn set_sound_settings(freesound: bool, key: Option<String>) -> CmdResult<SoundSettings> {
    let mut settings = sounds::settings();
    settings.freesound = freesound;
    if let Some(key) = key {
        settings.freesound_key = key;
    }
    sounds::save_settings(&settings).map_err(err)?;
    Ok(sound_settings())
}

/// Opens a sound's page or licence in the browser; only web pages open.
#[tauri::command]
pub fn open_sound_page(app: AppHandle, url: String) -> CmdResult<()> {
    if !url.starts_with("https://") || url.contains(char::is_whitespace) {
        return Err("Only web pages open from the sound library".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}
