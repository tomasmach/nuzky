//! Headless CapOpen: inspect media, render single frames and export projects.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use capopen_engine::edit::{EditCmd, new_id};
use capopen_engine::export::{ExportOptions, check_source_path, export};
use capopen_engine::media::probe;
use capopen_engine::{Project, Renderer, Wait};

mod style;

const USAGE: &str = "Usage:
  capopen mcp --project <path> [--allow-write] [--cache <dir>]
  capopen probe <media>
  capopen new <project.json> <media>...     main-track project from media files
  capopen frame <project.json> <seconds> <out.png> [width]
  capopen bench <project.json> [width] [seconds]
  capopen render <project.json> <out.mp4> [resolution] [fps]
";

fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("capopen")
}

fn load(path: &str) -> Result<Project> {
    let json = std::fs::read_to_string(path).with_context(|| format!("Cannot read {path}"))?;
    let project = serde_json::from_str(&json).with_context(|| format!("{path} is not a valid project"))?;
    capopen_session::validate(&project).with_context(|| format!("{path} is not a valid project"))?;
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
    let tmp = path.with_file_name(format!(".capopen-frame-{}.png", new_id()));
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
        for suffix in [".lock", ".checkpoint.json", ".tmp"] {
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
    let tmp = capopen_session::json_temp_path(out);
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

fn main() -> Result<()> {
    // Vulkan FP16 moves Whisper word times by up to 330 ms; FP32 is as fast. ggml reads this when
    // its backend starts, so it is set here, before any other thread exists.
    if std::env::var_os("GGML_VK_DISABLE_F16").is_none() {
        unsafe { std::env::set_var("GGML_VK_DISABLE_F16", "1") };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["mcp", ..] => capopen_mcp::bridge::run_args(&args[1..], cache_dir())?,
        ["probe", media] => {
            println!("{}", serde_json::to_string_pretty(&probe(Path::new(media), new_id())?)?);
        }
        ["new", out, media @ ..] if !media.is_empty() => {
            let out = Path::new(out);
            let _lock = capopen_session::lock_project(out, true)?;
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
            let width: u32 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(576);
            let seconds: f64 = rest.get(1).map(|s| s.parse()).transpose()?.unwrap_or(5.0);
            if !seconds.is_finite() || seconds <= 0.0 {
                bail!("Seconds must be positive");
            }
            let (width, height) = frame_size(&project, width)?;
            let fps = project.canvas.fps.max(1) as u64;
            let frames = ((seconds * fps as f64).ceil() as u64).max(1);
            let mut renderer = Renderer::new()?;
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
            let avg = times.iter().sum::<f64>() / times.len() as f64;
            times.sort_by(f64::total_cmp);
            println!(
                "{}: {width}x{height}, {frames} frames, avg {avg:.2} ms, p95 {:.2} ms, {} late layers",
                renderer.adapter_name(),
                times[(times.len() * 95).div_ceil(100).saturating_sub(1)],
                renderer.late_layers
            );
        }
        ["render", project, out, rest @ ..] => {
            check_render_output(Path::new(project), Path::new(out))?;
            let project = load(project)?;
            let start = Instant::now();
            let cancel = AtomicBool::new(false);
            let mut last = 0;
            let options = ExportOptions {
                resolution: rest.first().map(|s| s.parse()).transpose()?,
                fps: rest.get(1).map(|s| s.parse()).transpose()?,
                // The output path was typed on purpose, as with any command-line tool.
                replace_existing: true,
                ..ExportOptions::default()
            };
            export(&project, &cache_dir(), Path::new(out), &options, &cancel, |p| {
                let pct = p.frame * 100 / p.total_frames.max(1);
                if pct >= last + 10 {
                    last = pct;
                    eprintln!("{pct}%  ({}/{} frames)", p.frame, p.total_frames);
                }
            })?;
            eprintln!("Exported {out} in {:?}", start.elapsed());
        }
        ["style", ..] => style::run(&args[1..], &cache_dir())?,
        _ => bail!("{USAGE}{}", style::USAGE),
    }
    Ok(())
}
