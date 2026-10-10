//! Covers and YouTube thumbnails in the app: the cover the editor shows, drawn by the engine's
//! `render_thumbnail` exactly as the export draws it, and the jobs that pick its frame, mask the
//! person and export it. Each job reports through `job` events like the others.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use base64::Engine as _;
use nuzky_analysis::models_dir;
use nuzky_engine::edit::new_id;
use nuzky_engine::gpu::Image;
use nuzky_engine::model::{ClipContent, Project, Thumbnail, ThumbnailFormat, TrackKind};
use nuzky_engine::thumbnail::{ImageKind, RenderedThumbnail, picture};
use nuzky_engine::{Pending, Renderer, Wait};
use nuzky_vision::models::{self, Model};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::jobs::{Reporter, check_cancelled, panic_text, register, unregister};
use crate::{AppState, CmdResult, err};

/// The renderer of the covers the editor shows. It decodes the original files, as the export does;
/// the preview's renderer decodes the lighter proxies, whose frames would differ from the export's.
#[derive(Default)]
pub struct Covers {
    renderer: Option<Renderer>,
    frame: Option<Frame>,
}

/// The frame of the cover shown last, kept while the picture and time stay, so moving a text does
/// not decode the video again.
struct Frame {
    picture: Project,
    time_us: i64,
    image: Image,
    /// The person's mask, once it is in the cache.
    mask: Option<Vec<u8>>,
}

/// What the cover canvas shows besides its pixels.
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverView {
    pub width: u32,
    pub height: u32,
    pub time_us: i64,
    /// Each text's corners in thumbnail pixels (top-left, top-right, bottom-right, bottom-left); null
    /// for a text that is not drawn.
    pub bounds: Vec<Option<[[f32; 2]; 4]>>,
    /// The share of each text the person covers, 0..1.
    pub hidden: Vec<f32>,
    /// Where the frame is drawn, as for a clip: the corners of its visible part and of the whole frame.
    pub frame: [[[f32; 2]; 4]; 2],
    /// The cover cuts the person out of the frame and their mask is not made yet, so it is drawn
    /// without: text behind the person in front of them and no outline.
    pub mask_missing: bool,
}

/// The cover of `format` `width` pixels wide, as the export draws it: a little-endian u32 with the
/// length of the `CoverView` JSON, the JSON, then straight RGBA. `draft` draws a cover that is not in
/// the project, such as the one the editor offers before there is any. `refresh_mask` looks for the
/// mask in the cache again, once a job made it.
#[tauri::command]
pub async fn cover_view(
    app: AppHandle,
    format: ThumbnailFormat,
    width: u32,
    refresh_mask: bool,
    draft: Option<Thumbnail>,
) -> CmdResult<tauri::ipc::Response> {
    let mut project = app.state::<AppState>().project()?;
    if let Some(draft) = draft {
        if draft.format != format {
            return Err("The draft is a cover of another format".into());
        }
        project.thumbnails.retain(|t| t.format != format);
        project.thumbnails.push(draft);
        nuzky_session::validate(&project).map_err(err)?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut covers = state.covers.lock().unwrap();
        let (view, rgba) = covers.view(&project, format, width, refresh_mask, &state.cache_dir).map_err(err)?;
        let info = serde_json::to_vec(&view).map_err(err)?;
        let mut bytes = Vec::with_capacity(4 + info.len() + rgba.len());
        bytes.extend((info.len() as u32).to_le_bytes());
        bytes.extend(info);
        bytes.extend(rgba);
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
    .map_err(err)?
}

/// Frees the cover renderer when the cover editor closes. Off the UI thread: a cover still rendering holds the
/// renderer, and dropping it joins its decoders.
#[tauri::command]
pub async fn cover_close(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        let covers = std::mem::take(&mut *app.state::<AppState>().covers.lock().unwrap());
        drop(covers);
    })
    .await
    .ok();
}

impl Covers {
    fn view(
        &mut self,
        project: &Project,
        format: ThumbnailFormat,
        width: u32,
        refresh_mask: bool,
        cache: &Path,
    ) -> anyhow::Result<(CoverView, Vec<u8>)> {
        let thumbnail =
            project.thumbnail(format).context("THUMBNAIL_MISSING: this project has no cover in that format")?;
        let width = width.clamp(16, format.size().0);
        if let Some(name) = missing_at(project, thumbnail.time_us).first() {
            anyhow::bail!("MEDIA_MISSING: {name}");
        }
        if self.renderer.is_none() {
            let mut renderer = Renderer::new().context("Starting the cover renderer")?;
            // A clip whose person is not found yet shows as recorded, as in the preview.
            renderer.use_mattes(cache.to_path_buf(), Pending::Original);
            self.renderer = Some(renderer);
        }
        let renderer = self.renderer.as_mut().unwrap();
        // Only what the frame is drawn from: the cover's own texts and settings do not change it.
        let mut shown = picture(project);
        shown.thumbnails.clear();
        let fresh = !self.frame.as_ref().is_some_and(|f| f.time_us == thumbnail.time_us && f.picture == shown);
        if fresh {
            self.frame = None;
            let image = renderer.thumbnail_frame(project, thumbnail)?;
            let mask = nuzky_vision::cached_alpha(&image.data, (image.width, image.height), cache);
            self.frame = Some(Frame { picture: shown, time_us: thumbnail.time_us, image, mask });
        }
        let frame = self.frame.as_mut().unwrap();
        if refresh_mask && frame.mask.is_none() {
            frame.mask = nuzky_vision::cached_alpha(&frame.image.data, (frame.image.width, frame.image.height), cache);
        }
        let mask_missing = thumbnail.needs_mask() && frame.mask.is_none();
        let rendered = if mask_missing {
            // Until the mask is made, the whole frame stands in for the person.
            let draft = Thumbnail {
                texts: thumbnail
                    .texts
                    .iter()
                    .map(|t| nuzky_engine::model::ThumbnailText { behind: false, ..t.clone() })
                    .collect(),
                outline: None,
                ..thumbnail.clone()
            };
            let whole = vec![255; (frame.image.width * frame.image.height) as usize];
            renderer.render_thumbnail(&draft, &frame.image, Some(&whole), width)?
        } else {
            // The mask only where the export uses it too, so the two never differ.
            let mask = if thumbnail.needs_mask() { frame.mask.as_deref() } else { None };
            renderer.render_thumbnail(thumbnail, &frame.image, mask, width)?
        };
        let view = CoverView {
            width: rendered.width,
            height: rendered.height,
            time_us: thumbnail.time_us,
            bounds: rendered.bounds,
            hidden: if mask_missing { vec![0.0; thumbnail.texts.len()] } else { rendered.hidden },
            frame: rendered.frame_bounds,
            mask_missing,
        };
        Ok((view, rendered.rgba))
    }
}

/// Names of the missing files of the pictures shown at `t_us`.
fn missing_at(project: &Project, t_us: i64) -> Vec<String> {
    missing(project, Some(t_us))
}

/// Names of the missing files of the pictures, at `at` or anywhere on the timeline.
fn missing(project: &Project, at: Option<i64>) -> Vec<String> {
    let mut names = Vec::new();
    for track in project.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
        for clip in track.clips.iter().filter(|c| at.is_none_or(|t| c.start_us <= t && t < c.end_us())) {
            if let ClipContent::Media { asset_id, .. } = &clip.content
                && let Some(asset) = project.asset(asset_id)
                && !Path::new(&asset.path).is_file()
                && !names.contains(&asset.name)
            {
                names.push(asset.name.clone());
            }
        }
    }
    names
}

/// One frame worth a cover, for the filmstrip of the cover editor.
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverCandidate {
    pub time_us: i64,
    pub score: f32,
    /// Each 0..1, higher is better: sharpness and exposure, and with a face eyes_open, mouth, facing,
    /// smile, framing_9x16 and framing_16x9.
    pub parts: BTreeMap<String, f32>,
    pub face: bool,
    /// A JPEG data URL of the frame, about 240 pixels on its long side.
    pub still: String,
}

/// What Pick for me found, as the `output` of its job: candidates best first.
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverPick {
    pub format: ThumbnailFormat,
    pub candidates: Vec<CoverCandidate>,
}

/// The project as it is now, for a job; every file of its pictures must be there.
fn job_project(app: &AppHandle, expected_epoch: Option<&str>) -> CmdResult<Project> {
    let state = app.state::<AppState>();
    let project = crate::lock_session(&state.session, expected_epoch)?.host.session.state().map_err(err)?.project;
    if project.duration_us() <= 0 {
        return Err("Add a video to the timeline first.".into());
    }
    nuzky_vision::runtime::require().map_err(err)?;
    Ok(project)
}

/// Spawns a cover job: `run` gets the cancel flag and the reporter and returns the job's output.
fn spawn(
    app: &AppHandle,
    kind: &str,
    label: String,
    busy: &str,
    run: impl FnOnce(&AtomicBool, &mut Reporter) -> anyhow::Result<Option<String>> + Send + 'static,
) -> CmdResult<String> {
    let id = format!("{kind}:{}", new_id());
    let cancel = register(app, &id).ok_or_else(|| busy.to_owned())?;
    let (worker, job) = (app.clone(), id.clone());
    let spawned = std::thread::Builder::new().name(kind.into()).spawn(move || {
        let mut rep = Reporter::new(&worker, &job, "cover", label);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&cancel, &mut rep)))
            .unwrap_or_else(|p| Err(anyhow::anyhow!("The cover job crashed: {}", panic_text(&p))));
        rep.finish(result, cancel.load(Ordering::Relaxed));
        unregister(&worker, &job);
    });
    if let Err(error) = spawned {
        unregister(app, &id);
        return Err(format!("Starting the cover job: {error}"));
    }
    Ok(id)
}

/// Downloads the models that are missing, each checked against its pinned SHA-256. `repair` checks the
/// installed ones too and downloads again any whose contents are damaged.
pub fn download_models(
    list: &[Model],
    repair: bool,
    phase: &str,
    cancel: &AtomicBool,
    rep: &mut Reporter,
) -> anyhow::Result<()> {
    let dir = models_dir();
    let missing = if repair { list.to_vec() } else { models::missing(list, &dir) };
    let total = missing.iter().map(|m| m.size).sum::<u64>().max(1) as f32;
    let mut before = 0;
    for model in &missing {
        let integrity = nuzky_mcp::model_download::Integrity { size: model.size, sha256: model.sha256 };
        nuzky_mcp::model_download::download(model.url, &model.path(&dir), integrity, cancel, |part| {
            rep.progress((before as f32 + part * model.size as f32) / total, Some(phase))
        })?;
        before += model.size;
    }
    Ok(())
}

/// Finds the person behind whom the project's clip backgrounds go, downloading the person model first when they
/// need it, so the cover's frame is drawn as the video exports it.
fn prepare_mattes(project: &Project, cache: &Path, cancel: &AtomicBool, rep: &mut Reporter) -> anyhow::Result<()> {
    if nuzky_engine::matte::missing(cache, project).is_empty() {
        return Ok(());
    }
    download_models(models::BACKGROUND, false, "Downloading the person model", cancel, rep)?;
    nuzky_vision::matte::prepare(project, cache, &models_dir(), cancel, &mut |p| {
        rep.progress(p, Some("Finding the person"))
    })
}

fn mask_progress(rep: &mut Reporter) -> impl FnMut(nuzky_vision::mask::Phase) + '_ {
    |phase| {
        let label = match phase {
            nuzky_vision::mask::Phase::Waiting => "Waiting for another mask",
            _ => "Finding the person",
        };
        rep.progress(0.0, Some(label))
    }
}

/// Pick for me: downloads the models on first use, picks the best frames of the whole timeline for
/// `format` and masks the person in the best one, so text can go behind them at once.
#[tauri::command]
pub fn start_cover_pick(app: AppHandle, format: ThumbnailFormat, expected_epoch: Option<String>) -> CmdResult<String> {
    let project = job_project(&app, expected_epoch.as_deref())?;
    if let Some(name) = missing(&project, None).first() {
        return Err(format!("MEDIA_MISSING: {name}"));
    }
    let cache = app.state::<AppState>().cache_dir.clone();
    spawn(
        &app,
        "cover-pick",
        "Picking a cover frame".into(),
        "A cover frame is already being picked",
        move |cancel, rep| {
            download_models(models::ALL, false, "Downloading cover models", cancel, rep)?;
            prepare_mattes(&project, &cache, cancel, rep)?;
            let dir = models_dir();
            let vision = match format {
                ThumbnailFormat::Cover9x16 => nuzky_vision::Format::Vertical,
                ThumbnailFormat::Youtube16x9 => nuzky_vision::Format::Wide,
            };
            // Which frame suits a cover does not depend on what is behind the person.
            let plain = nuzky_engine::matte::without_backgrounds(&project);
            let found = nuzky_vision::thumbnail_frames(&plain, &dir, Some(vision), cancel, &mut |phase, done| {
                let label = match phase {
                    nuzky_vision::thumbnails::Phase::Looking => "Looking through the video",
                    nuzky_vision::thumbnails::Phase::Scoring => "Scoring frames",
                };
                rep.progress(done, Some(label))
            })?;
            let shown = picture(&project);
            let mut renderer = Renderer::new().context("Starting the cover renderer")?;
            renderer.use_mattes(cache.clone(), Pending::Fail);
            let (w, h) = still_size(&project);
            let mut candidates = Vec::with_capacity(found.len());
            for candidate in &found {
                check_cancelled(cancel)?;
                let rgba = renderer.render(&shown, candidate.time_us, w, h, Wait::Exact, false)?;
                candidates.push(CoverCandidate {
                    time_us: candidate.time_us,
                    score: candidate.score,
                    parts: candidate.parts.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
                    face: !candidate.faces.is_empty(),
                    still: jpeg_url(w, h, rgba)?,
                });
            }
            drop(renderer);
            if let Some(best) = found.first() {
                nuzky_vision::segment_subject(&project, best.time_us, &dir, &cache, cancel, &mut mask_progress(rep))?;
            }
            Ok(Some(serde_json::to_string(&CoverPick { format, candidates })?))
        },
    )
}

/// About 240 pixels on the long side, in the canvas's shape.
fn still_size(project: &Project) -> (u32, u32) {
    let (w, h) = (project.canvas.width.max(1) as f32, project.canvas.height.max(1) as f32);
    let k = 240.0 / w.max(h);
    ((((w * k).round() as u32).max(16)) & !1, (((h * k).round() as u32).max(16)) & !1)
}

fn jpeg_url(width: u32, height: u32, rgba: Vec<u8>) -> anyhow::Result<String> {
    let rendered = RenderedThumbnail {
        width,
        height,
        rgba,
        hidden: Vec::new(),
        bounds: Vec::new(),
        frame_bounds: [[[0.0; 2]; 4]; 2],
    };
    let bytes = nuzky_engine::thumbnail::encode(&rendered, ImageKind::Jpeg)?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Masks the person in the timeline frame at `time_us`, downloading the mask model on first use.
#[tauri::command]
pub fn start_cover_mask(app: AppHandle, time_us: i64, expected_epoch: Option<String>) -> CmdResult<String> {
    let project = job_project(&app, expected_epoch.as_deref())?;
    if !(0..project.duration_us()).contains(&time_us) {
        return Err("The cover's frame is past the end of the video. Choose its frame again.".into());
    }
    if let Some(name) = missing_at(&project, time_us).first() {
        return Err(format!("MEDIA_MISSING: {name}"));
    }
    let cache = app.state::<AppState>().cache_dir.clone();
    spawn(&app, "cover-mask", "Finding the person".into(), "The person is already being found", move |cancel, rep| {
        download_models(models::MASK, false, "Downloading cover models", cancel, rep)?;
        prepare_mattes(&project, &cache, cancel, rep)?;
        let mask =
            nuzky_vision::segment_subject(&project, time_us, &models_dir(), &cache, cancel, &mut mask_progress(rep))?;
        Ok(Some(serde_json::json!({"person": mask.person, "found": mask.subject_box.is_some()}).to_string()))
    })
}

/// Writes the cover of `format` at its full size to `path` (`.png`, `.jpg` or `.jpeg`). Without
/// `replace_existing` an existing file is kept and this fails with DESTINATION_EXISTS.
#[tauri::command]
pub fn start_cover_export(
    app: AppHandle,
    format: ThumbnailFormat,
    path: String,
    replace_existing: bool,
    expected_epoch: Option<String>,
) -> CmdResult<String> {
    let out = PathBuf::from(path);
    let kind = ImageKind::of(&out).ok_or("Save the cover as a .png or .jpg file.")?;
    crate::jobs::check_destination(&out, replace_existing)?;
    let project = crate::lock_session(&app.state::<AppState>().session, expected_epoch.as_deref())?
        .host
        .session
        .state()
        .map_err(err)?
        .project;
    let thumbnail = project.thumbnail(format).cloned().ok_or("Make the cover first.")?;
    nuzky_engine::export::check_source_path(&project, &out).map_err(err)?;
    if let Some(name) = missing_at(&project, thumbnail.time_us).first() {
        return Err(format!("MEDIA_MISSING: {name}"));
    }
    let cache = app.state::<AppState>().cache_dir.clone();
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    spawn(&app, "cover-export", format!("Exporting {name}"), "A cover is already being exported", move |cancel, rep| {
        prepare_mattes(&project, &cache, cancel, rep)?;
        rep.progress(0.0, Some("Rendering"));
        let mut renderer = Renderer::new().context("Starting the cover renderer")?;
        renderer.use_mattes(cache.clone(), Pending::Fail);
        let frame = renderer.thumbnail_frame(&project, &thumbnail)?;
        let mask = if thumbnail.needs_mask() {
            download_models(models::MASK, false, "Downloading cover models", cancel, rep)?;
            let size = (frame.width, frame.height);
            let (alpha, _) =
                nuzky_vision::subject_alpha(&frame.data, size, &models_dir(), &cache, cancel, &mut mask_progress(rep))?;
            Some(alpha)
        } else {
            None
        };
        check_cancelled(cancel)?;
        rep.progress(0.9, Some("Rendering"));
        let rendered = renderer.render_thumbnail(&thumbnail, &frame, mask.as_deref(), format.size().0)?;
        let bytes = nuzky_engine::thumbnail::encode(&rendered, kind)?;
        nuzky_engine::thumbnail::save(&out, &bytes, replace_existing, cancel)?;
        Ok(Some(out.to_string_lossy().into_owned()))
    })
}
