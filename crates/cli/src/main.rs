//! Headless Nuzky: inspect media, render single frames and export projects.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use nuzky_engine::edit::{EditCmd, new_id};
use nuzky_engine::export::{Delivery, ExportOptions, check_source_path, export};
use nuzky_engine::media::probe;
use nuzky_engine::model::{AssetKind, ThumbnailFormat};
use nuzky_engine::thumbnail::ImageKind;
use nuzky_engine::{Project, Renderer, Wait, proxy};

mod style;

const USAGE: &str = "Usage:
  nuzky mcp (--project <path> | --current) [--allow-write] [--cache <dir>]
  nuzky probe <media>
  nuzky new <project.json> <media>...     main-track project from media files
  nuzky frame <project.json> <seconds> <out.png> [width]
  nuzky bench <project.json> [width] [seconds] [--proxy]
      plays and scrubs the preview; --proxy first makes the preview proxies of heavy video and plays from them
  nuzky render <project.json> <out.mp4> [resolution] [fps] [--preset reels]
      reels: Instagram Reels and TikTok, 1080x1920 at 30 fps, sound levelled to -14 LUFS (9:16 only)
  nuzky vision-models                     download the face and subject models
  nuzky thumbnail-frames <project.json> [--format 9:16|16:9]
      frames worth a cover, best first, as JSON; downloads missing face models first
  nuzky mask <project.json> <seconds> <out.png>
      the subject of that frame as a grayscale alpha PNG; downloads the mask model first
  nuzky thumbnail <project.json> cover_9x16|youtube_16x9 <out.png|out.jpg>
      the project's cover or YouTube thumbnail at full size; downloads the mask model first when it needs one
";

/// `[resolution] [fps]` and an optional `--preset <name>` anywhere among them.
fn render_options(rest: &[&str]) -> Result<ExportOptions> {
    let mut delivery = None;
    let mut positional = Vec::new();
    let mut args = rest.iter();
    while let Some(&arg) = args.next() {
        if arg == "--preset" {
            let name = args.next().context("--preset needs a name: reels")?;
            delivery = Some(match *name {
                "reels" => Delivery::Reels,
                other => bail!("Unknown preset {other}; the preset is reels"),
            });
        } else {
            positional.push(arg);
        }
    }
    ensure!(positional.len() <= 2, "{USAGE}");
    Ok(ExportOptions {
        resolution: positional.first().map(|s| s.parse()).transpose().context("Resolution must be a number")?,
        fps: positional.get(1).map(|s| s.parse()).transpose().context("Frame rate must be a number")?,
        delivery,
        // The output path was typed on purpose, as with any command-line tool.
        replace_existing: true,
        replace_credits: true,
        ..ExportOptions::default()
    })
}

fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nuzky")
}

fn load(path: &str) -> Result<Project> {
    let json = std::fs::read_to_string(path).with_context(|| format!("Cannot read {path}"))?;
    let project = serde_json::from_str(&json).with_context(|| format!("{path} is not a valid project"))?;
    nuzky_session::validate(&project).with_context(|| format!("{path} is not a valid project"))?;
    Ok(project)
}

/// Frames must fit the renderer's textures, like the canvas itself.
fn frame_size(project: &Project, width: u32) -> Result<(u32, u32)> {
    const SIDE: std::ops::RangeInclusive<u32> = 16..=7680;
    ensure!(SIDE.contains(&width), "Frame width must be 16..=7680 pixels");
    let height = (width as u64 * project.canvas.height as u64 / project.canvas.width as u64) as u32;
    ensure!(SIDE.contains(&height), "Frame height {height} must be 16..=7680 pixels; use another width");
    Ok((width, height))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.write_header()?.write_image_data(rgba)?;
    // Renaming over the destination leaves a hard-linked source file untouched.
    let tmp = path.with_file_name(format!(".nuzky-frame-{}.png", new_id()));
    let result = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("Cannot write {}", path.display()))
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    const MAX_SYMLINKS: usize = 40;
    let mut path = path.to_path_buf();
    for _ in 0..MAX_SYMLINKS {
        if let Ok(target) = std::fs::canonicalize(&path) {
            return Ok(target);
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        match std::fs::read_link(&path) {
            Ok(target) => path = if target.is_absolute() { target } else { parent.join(target) },
            Err(_) => {
                return Ok(std::fs::canonicalize(parent)?.join(path.file_name().context("Output needs a filename")?));
            }
        }
    }
    bail!("Too many symlinks in output path")
}

/// Resolve the parent while preserving the final directory entry, which export replaces.
fn absolute_entry(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(parent)?.join(path.file_name().context("Output needs a filename")?))
}

fn check_render_output(project: &Path, out: &Path) -> Result<()> {
    let targets = [resolved_path(out)?, absolute_entry(out)?];
    let canonical = std::fs::canonicalize(project)?;
    for base in [absolute_entry(project)?, canonical] {
        let mut prefix = base.file_name().context("Project needs a filename")?.to_os_string();
        prefix.push(".");
        for target in &targets {
            if target == &base
                || (target.parent() == base.parent()
                    && target
                        .file_name()
                        .is_some_and(|name| name.as_encoded_bytes().starts_with(prefix.as_encoded_bytes())))
                || target == &resolved_path(&base.with_extension("tmp"))?
            {
                bail!("Output would overwrite the project or its sidecar: {}", out.display());
            }
        }
        // Preserve protection of existing sidecars whose own symlinks point outside that namespace.
        for suffix in [".lock", ".checkpoint.json", nuzky_session::HISTORY_SUFFIX, ".tmp"] {
            let mut path = base.as_os_str().to_os_string();
            path.push(suffix);
            if targets.contains(&resolved_path(Path::new(&path))?) {
                bail!("Output would overwrite the project or its sidecar: {}", out.display());
            }
        }
    }
    Ok(())
}

fn publish_new_project(out: &Path, project: &Project) -> Result<()> {
    let tmp = nuzky_session::json_temp_path(out);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(project)?)?;
        file.sync_all()?;
        // Unlike rename, hard_link refuses a destination created since the existence check.
        match std::fs::hard_link(&tmp, out) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(anyhow::Error::new(error).context("Publishing new project"));
            }
            // FAT32 and exFAT have no hard links: claim the name first, so nothing is replaced.
            Err(_) => {
                std::fs::OpenOptions::new().write(true).create_new(true).open(out).context("Publishing new project")?;
                if let Err(error) = std::fs::rename(&tmp, out) {
                    let _ = std::fs::remove_file(out);
                    return Err(anyhow::Error::new(error).context("Publishing new project"));
                }
            }
        }
        Ok(())
    })();
    drop(file);
    let _ = std::fs::remove_file(tmp);
    result
}

/// Downloads whichever of `models` are missing, checking size and SHA-256, once ONNX Runtime has
/// loaded: models that could not run are not worth downloading.
fn install_models(models: &[nuzky_vision::models::Model]) -> Result<()> {
    nuzky_vision::runtime::require()?;
    let dir = nuzky_analysis::models_dir();
    let cancel = AtomicBool::new(false);
    for model in nuzky_vision::models::missing(models, &dir) {
        eprintln!("Downloading the {} ({} MB)", model.label, model.size.div_ceil(1_000_000));
        let mut shown = 0;
        let integrity = nuzky_mcp::model_download::Integrity { size: model.size, sha256: model.sha256 };
        nuzky_mcp::model_download::download(model.url, &model.path(&dir), integrity, &cancel, |done| {
            let pct = (done * 100.0) as u32;
            if pct >= shown + 10 {
                shown = pct;
                eprintln!("{pct}%");
            }
        })?;
    }
    Ok(())
}

/// Playhead jumps the bench scrubs, spread over the timeline out of order.
const SCRUB_JUMPS: i64 = 20;

/// Average and 95th percentile, sorting the times.
fn summary(times: &mut [f64]) -> (f64, f64) {
    let avg = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    (avg, times[(times.len() * 95).div_ceil(100).saturating_sub(1)])
}

/// Prints how long each phase of a job took, to stderr.
struct PhaseClock<P> {
    phase: Option<(P, Instant)>,
}

impl<P: PartialEq + Copy + std::fmt::Debug> PhaseClock<P> {
    fn enter(&mut self, phase: P) {
        if self.phase.is_some_and(|(current, _)| current == phase) {
            return;
        }
        self.finish();
        self.phase = Some((phase, Instant::now()));
    }

    fn finish(&mut self) {
        if let Some((phase, since)) = self.phase.take() {
            eprintln!("{phase:?}: {:.2} s", since.elapsed().as_secs_f64());
        }
    }
}

fn main() -> Result<()> {
    // Vulkan FP16 moves Whisper word times by up to 330 ms; FP32 is as fast. ggml reads this when
    // its backend starts, so it is set here, before any other thread exists.
    if std::env::var_os("GGML_VK_DISABLE_F16").is_none() {
        unsafe { std::env::set_var("GGML_VK_DISABLE_F16", "1") };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["mcp", ..] => nuzky_mcp::bridge::run_args(&args[1..], cache_dir())?,
        ["probe", media] => {
            println!("{}", serde_json::to_string_pretty(&probe(Path::new(media), new_id())?)?);
        }
        ["new", out, media @ ..] if !media.is_empty() => {
            let out = Path::new(out);
            let _lock = nuzky_session::lock_project(out, true)?;
            if out.symlink_metadata().is_ok() {
                bail!("Project already exists: {}", out.display());
            }
            let mut project = Project::new("CLI project");
            for m in media {
                let asset = probe(Path::new(m), new_id())?;
                let id = asset.id.clone();
                project.apply(EditCmd::AddAssets { assets: vec![asset] })?;
                project.apply(EditCmd::AddClip { asset_id: id, start_us: None, track_id: None })?;
            }
            publish_new_project(out, &project)?;
        }
        ["frame", project, secs, out, rest @ ..] => {
            check_render_output(Path::new(project), Path::new(out))?;
            let project = load(project)?;
            check_source_path(&project, Path::new(out))?;
            let width: u32 = rest.first().map(|w| w.parse()).transpose()?.unwrap_or(project.canvas.width);
            let (width, height) = frame_size(&project, width)?;
            let t = (secs.parse::<f64>()? * 1e6) as i64;
            let mut renderer = Renderer::new()?;
            let start = Instant::now();
            let rgba = renderer.render(&project, t, width, height, Wait::Exact, false)?;
            eprintln!("Rendered {width}x{height} at {secs}s in {:?} on {}", start.elapsed(), renderer.adapter_name());
            write_png(Path::new(out), width, height, &rgba)?;
        }
        ["bench", project, rest @ ..] => {
            let project = load(project)?;
            let proxies = rest.contains(&"--proxy");
            let rest: Vec<&str> = rest.iter().copied().filter(|arg| *arg != "--proxy").collect();
            let width: u32 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(576);
            let seconds: f64 = rest.get(1).map(|s| s.parse()).transpose()?.unwrap_or(5.0);
            if !seconds.is_finite() || seconds <= 0.0 {
                bail!("Seconds must be positive");
            }
            let (width, height) = frame_size(&project, width)?;
            let fps = project.canvas.fps.max(1) as u64;
            let frames = ((seconds * fps as f64).ceil() as u64).max(1);
            let mut renderer = Renderer::new()?;
            if proxies {
                for asset in project.assets.iter().filter(|a| a.kind == AssetKind::Video) {
                    let source = Path::new(&asset.path);
                    if proxy::wanted(source)? {
                        let start = Instant::now();
                        proxy::ensure_proxy(&cache_dir(), source, |_| Ok(()))?;
                        eprintln!("Proxy of {} ready in {:.1} s", asset.name, start.elapsed().as_secs_f64());
                    }
                }
                renderer.use_proxies(cache_dir());
            }
            renderer.render(&project, 0, width, height, Wait::Exact, true)?;
            let clock = Instant::now();
            let mut times = Vec::with_capacity(frames as usize);
            for i in 0..frames {
                let deadline = clock + std::time::Duration::from_secs_f64(i as f64 / fps as f64);
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                let now = Instant::now();
                renderer.render(&project, (i * 1_000_000 / fps) as i64, width, height, Wait::Ready, true)?;
                times.push(now.elapsed().as_secs_f64() * 1000.0);
            }
            let (avg, p95) = summary(&mut times);
            let text = renderer.text_stats();
            println!(
                "{}: {width}x{height}, {frames} frames, avg {avg:.2} ms, p95 {p95:.2} ms, {} late layers, text laid out {} times and painted {} times",
                renderer.adapter_name(),
                renderer.late_layers,
                text.layouts,
                text.paints
            );
            // Scrubbing: the paused preview waits for the exact frame wherever the playhead lands.
            let mut jumps = Vec::with_capacity(SCRUB_JUMPS as usize);
            for i in 0..SCRUB_JUMPS {
                let t = project.duration_us() * ((i * 7) % SCRUB_JUMPS) / SCRUB_JUMPS + 12_345;
                let now = Instant::now();
                renderer.render(&project, t.min(project.duration_us() - 1), width, height, Wait::Exact, false)?;
                jumps.push(now.elapsed().as_secs_f64() * 1000.0);
            }
            let (avg, p95) = summary(&mut jumps);
            println!("scrub: {SCRUB_JUMPS} jumps, avg {avg:.2} ms, p95 {p95:.2} ms");
        }
        ["render", project, out, rest @ ..] => {
            let options = render_options(rest)?;
            check_render_output(Path::new(project), Path::new(out))?;
            let project = load(project)?;
            let start = Instant::now();
            let cancel = AtomicBool::new(false);
            let mut last = None;
            export(&project, &cache_dir(), Path::new(out), &options, &cancel, |p| {
                let pct = (p.fraction * 100.0) as u32;
                if last.is_none_or(|(phase, at)| phase != p.phase || pct >= at + 10) {
                    last = Some((p.phase, pct));
                    eprintln!("{pct}%  {} ({}/{} frames)", p.phase.label(), p.frame, p.total_frames);
                }
            })?;
            eprintln!("Exported {out} in {:?}", start.elapsed());
        }
        ["vision-models"] => install_models(nuzky_vision::models::ALL)?,
        ["thumbnail-frames", project, rest @ ..] => {
            let format = match rest {
                [] => None,
                ["--format", "9:16"] => Some(nuzky_vision::Format::Vertical),
                ["--format", "16:9"] => Some(nuzky_vision::Format::Wide),
                _ => bail!("{USAGE}"),
            };
            let project = load(project)?;
            install_models(nuzky_vision::models::FRAMES)?;
            let (start, cancel) = (Instant::now(), AtomicBool::new(false));
            let mut clock = PhaseClock { phase: None };
            let candidates = nuzky_vision::thumbnail_frames(
                &project,
                &nuzky_analysis::models_dir(),
                format,
                &cancel,
                &mut |phase, _| clock.enter(phase),
            )?;
            clock.finish();
            eprintln!("{} candidates in {:.2} s", candidates.len(), start.elapsed().as_secs_f64());
            println!("{}", serde_json::to_string_pretty(&candidates)?);
        }
        ["mask", project, secs, out] => {
            check_render_output(Path::new(project), Path::new(out))?;
            let project = load(project)?;
            check_source_path(&project, Path::new(out))?;
            install_models(nuzky_vision::models::MASK)?;
            let t = (secs.parse::<f64>()? * 1e6) as i64;
            let (start, cancel) = (Instant::now(), AtomicBool::new(false));
            let mut clock = PhaseClock { phase: None };
            let mask = nuzky_vision::segment_subject(
                &project,
                t,
                &nuzky_analysis::models_dir(),
                &cache_dir(),
                &cancel,
                &mut |phase| clock.enter(phase),
            )?;
            clock.finish();
            eprintln!("Mask in {:.2} s{}", start.elapsed().as_secs_f64(), if mask.cached { " (cached)" } else { "" });
            let bytes = std::fs::read(&mask.path).context("Reading the cached mask")?;
            // Renaming over the destination leaves a hard-linked source file untouched.
            let out = Path::new(out);
            let tmp = out.with_file_name(format!(".nuzky-mask-{}.png", new_id()));
            let written = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, out));
            if written.is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
            written.with_context(|| format!("Cannot write {}", out.display()))?;
            println!("{}", serde_json::to_string_pretty(&mask)?);
        }
        ["thumbnail", project, format, out] => {
            let format: ThumbnailFormat = serde_json::from_value(serde_json::json!(format))
                .context("The format is cover_9x16 or youtube_16x9")?;
            let kind = ImageKind::of(Path::new(out)).context("The output must end in .png, .jpg or .jpeg")?;
            check_render_output(Path::new(project), Path::new(out))?;
            let project = load(project)?;
            check_source_path(&project, Path::new(out))?;
            let thumbnail = project.thumbnail(format).context("The project has no thumbnail in this format")?;
            let mut renderer = Renderer::new()?;
            let frame = renderer.thumbnail_frame(&project, thumbnail)?;
            let cancel = AtomicBool::new(false);
            let mask = if thumbnail.needs_mask() {
                install_models(nuzky_vision::models::MASK)?;
                let mut clock = PhaseClock { phase: None };
                let size = (frame.width, frame.height);
                let models = nuzky_analysis::models_dir();
                let (alpha, _) =
                    nuzky_vision::subject_alpha(&frame.data, size, &models, &cache_dir(), &cancel, &mut |phase| {
                        clock.enter(phase)
                    })?;
                clock.finish();
                Some(alpha)
            } else {
                None
            };
            let rendered = renderer.render_thumbnail(thumbnail, &frame, mask.as_deref(), format.size().0)?;
            let bytes = nuzky_engine::thumbnail::encode(&rendered, kind)?;
            // The output path was typed on purpose, as with any command-line tool.
            nuzky_engine::thumbnail::save(Path::new(out), &bytes, true, &cancel)?;
            let texts: Vec<_> = thumbnail
                .texts
                .iter()
                .zip(&rendered.hidden)
                .map(|(t, hidden)| serde_json::json!({"text": t.text, "behind": t.behind, "hidden": hidden}))
                .collect();
            let result = serde_json::json!({"width": rendered.width, "height": rendered.height, "bytes": bytes.len(), "texts": texts});
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ["style", ..] => style::run(&args[1..], &cache_dir())?,
        _ => bail!("{USAGE}{}", style::USAGE),
    }
    Ok(())
}
