//! Headless CapOpen: inspect media, render single frames and export projects.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use capopen_engine::edit::{EditCmd, new_id};
use capopen_engine::export::{ExportOptions, export};
use capopen_engine::media::probe;
use capopen_engine::{Project, Renderer, Wait};

const USAGE: &str = "Usage:
  capopen mcp --project <path> [--allow-write] [--cache <dir>]
  capopen probe <media>
  capopen new <project.json> <media>...     main-track project from media files
  capopen frame <project.json> <seconds> <out.png> [width]
  capopen bench <project.json> [width] [seconds]
  capopen render <project.json> <out.mp4> [resolution] [fps]";

fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("capopen")
}

fn load(path: &str) -> Result<Project> {
    let json = std::fs::read_to_string(path).with_context(|| format!("Cannot read {path}"))?;
    serde_json::from_str(&json).with_context(|| format!("{path} is not a valid project"))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    const MAX_SYMLINKS: usize = 40;
    let mut path = path.to_path_buf();
    for _ in 0..MAX_SYMLINKS {
        if let Ok(target) = std::fs::canonicalize(&path) { return Ok(target); }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        match std::fs::read_link(&path) {
            Ok(target) => path = if target.is_absolute() { target } else { parent.join(target) },
            Err(_) => return Ok(std::fs::canonicalize(parent)?.join(path.file_name().context("Output needs a filename")?)),
        }
    }
    bail!("Too many symlinks in output path")
}

fn check_render_output(project: &Path, out: &Path) -> Result<()> {
    let target = resolved_path(out)?;
    let canonical = std::fs::canonicalize(project)?;
    for base in [project, canonical.as_path()] {
        for suffix in ["", ".lock", ".checkpoint.json", ".tmp"] {
            let mut path = base.as_os_str().to_os_string();
            path.push(suffix);
            if target == resolved_path(Path::new(&path))? {
                bail!("Output would overwrite the project or its sidecar: {}", out.display());
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    // Vulkan FP16 moves Whisper word times by up to 330 ms; FP32 is as fast. ggml reads this when
    // its backend starts, so it is set here, before any other thread exists.
    if std::env::var_os("GGML_VK_DISABLE_F16").is_none() {
        unsafe { std::env::set_var("GGML_VK_DISABLE_F16", "1") };
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["mcp", rest @ ..] => serve_mcp(rest)?,
        ["probe", media] => {
            println!("{}", serde_json::to_string_pretty(&probe(Path::new(media), new_id())?)?);
        }
        ["new", out, media @ ..] if !media.is_empty() => {
            let mut project = Project::new("CLI project");
            for m in media {
                let asset = probe(Path::new(m), new_id())?;
                let id = asset.id.clone();
                project.apply(EditCmd::AddAssets { assets: vec![asset] })?;
                project.apply(EditCmd::AddClip { asset_id: id, start_us: None, track_id: None })?;
            }
            std::fs::write(out, serde_json::to_string_pretty(&project)?)?;
        }
        ["frame", project, secs, out, rest @ ..] => {
            let project = load(project)?;
            let width: u32 = rest.first().map(|w| w.parse()).transpose()?.unwrap_or(project.canvas.width);
            let height = (width as u64 * project.canvas.height as u64 / project.canvas.width as u64) as u32;
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
            if width == 0 || !seconds.is_finite() || seconds <= 0.0 {
                bail!("Width and seconds must be positive");
            }
            let height = (width as u64 * project.canvas.height as u64 / project.canvas.width.max(1) as u64) as u32;
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
            println!("{}: {width}x{height}, {frames} frames, avg {avg:.2} ms, p95 {:.2} ms, {} late layers", renderer.adapter_name(), times[(times.len() * 95).div_ceil(100).saturating_sub(1)], renderer.late_layers);
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
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

fn serve_mcp(args: &[&str]) -> Result<()> {
    let mut project = None;
    let mut cache = cache_dir();
    let mut allow_write = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--project" => project = Some(PathBuf::from(args.next().context("--project requires a path")?)),
            "--cache" => cache = PathBuf::from(args.next().context("--cache requires a directory")?),
            "--allow-write" => allow_write = true,
            _ => bail!("Unknown MCP argument: {arg}\n{USAGE}"),
        }
    }
    let project = project.context("mcp requires --project <path>")?;
    capopen_mcp::bridge::run(&project, allow_write, cache)
}
