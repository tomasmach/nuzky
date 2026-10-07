//! MP4 export: the preview renderer at canvas resolution, H.264 + AAC.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use ff::software::scaling;
use ff::util::{color, format::Pixel, frame};
use ffmpeg_next as ff;

use crate::audio::{Mixer, ensure_pcm, has_audio};
use crate::media::{init, set_sws_colorspace};
use crate::model::{CHANNELS, ClipContent, Project, SAMPLE_RATE, TrackKind};
use crate::render::{Renderer, Wait};

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    High,
    Recommended,
    Small,
}

impl Quality {
    pub fn crf(self) -> u8 {
        match self {
            Self::High => 17,
            Self::Recommended => 21,
            Self::Small => 26,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub crf: u8,
    pub replace_existing: bool,
    pub preset: String,
    /// Short side of the output in pixels (720, 1080, 1440, 2160); `None` keeps the canvas size.
    pub resolution: Option<u32>,
    /// Output frame rate; `None` uses the project frame rate.
    pub fps: Option<u32>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            crf: Quality::Recommended.crf(),
            replace_existing: false,
            preset: "veryfast".into(),
            resolution: None,
            fps: None,
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub frame: u64,
    pub total_frames: u64,
}

/// Renders the whole timeline to `out`. Returns early with an error if `cancel` is set.
pub fn export(
    project: &Project,
    cache_dir: &Path,
    out: &Path,
    options: &ExportOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ExportProgress),
) -> Result<()> {
    init();
    let duration = project.duration_us();
    if duration <= 0 {
        bail!("The timeline is empty");
    }
    check_source_path(project, out)?;
    let heard: std::collections::HashSet<&str> = project
        .tracks
        .iter()
        .filter(|track| !track.muted && track.kind != TrackKind::Text)
        .flat_map(|track| &track.clips)
        .filter_map(|clip| match &clip.content {
            ClipContent::Media { asset_id, volume, .. }
                if *volume > 0.0 && clip.end_us() > 0 && clip.start_us < duration =>
            {
                Some(asset_id.as_str())
            }
            _ => None,
        })
        .collect();
    for asset in project.assets.iter().filter(|a| heard.contains(a.id.as_str()) && has_audio(a)) {
        ensure_pcm(cache_dir, asset, |_| Ok(()))?;
    }
    // Reserve beside the destination so rename stays on the same filesystem.
    let tmp = loop {
        let path = out.with_file_name(format!(".capopen-part-{}.mp4", uuid::Uuid::new_v4()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => break path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("Cannot reserve temporary export file"),
        }
    };
    let result = encode(project, cache_dir, &tmp, options, cancel, &mut progress, duration)
        .and_then(|()| publish(&tmp, out, options.replace_existing, cancel));
    if result.is_err() {
        // Report why the export failed, not a secondary cleanup problem.
        if let Err(e) = std::fs::remove_file(&tmp) {
            log::warn!("Cannot remove temporary export {}: {e}", tmp.display());
        }
    }
    result
}

pub fn check_source_path(project: &Project, path: &Path) -> Result<()> {
    if let Ok(target) = std::fs::canonicalize(path)
        && project.assets.iter().any(|a| std::fs::canonicalize(&a.path).ok().as_ref() == Some(&target))
    {
        bail!("{} is used in this project. Choose another file name.", path.display());
    }
    Ok(())
}

fn publish(tmp: &Path, out: &Path, replace_existing: bool, cancel: &AtomicBool) -> Result<()> {
    publish_with(tmp, out, replace_existing, cancel, |from, to| std::fs::hard_link(from, to))
}

/// `link` is `hard_link`, replaceable in tests by a filesystem without hard links.
fn publish_with(
    tmp: &Path,
    out: &Path,
    replace_existing: bool,
    cancel: &AtomicBool,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("CANCELLED: export cancelled");
    }
    let cannot_write = |error: std::io::Error| anyhow!(error).context(format!("Cannot write {}", out.display()));
    if replace_existing {
        return std::fs::rename(tmp, out).map_err(cannot_write);
    }
    let exists = || anyhow!("OUTPUT_EXISTS: choose a new export path");
    // Linking on the same filesystem atomically refuses an existing destination.
    match link(tmp, out) {
        Ok(()) => std::fs::remove_file(tmp).context("Removing published export temporary file")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(exists()),
        // FAT32 and exFAT, e.g. USB sticks, have no hard links. Claiming the name first still
        // never replaces a file that was there; the rename then swaps in the export.
        Err(error) => {
            log::debug!("Cannot hard link {} ({error}); publishing by rename", out.display());
            match std::fs::OpenOptions::new().write(true).create_new(true).open(out) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(exists()),
                Err(error) => return Err(cannot_write(error)),
            }
            if let Err(error) = std::fs::rename(tmp, out) {
                std::fs::remove_file(out).ok();
                return Err(cannot_write(error));
            }
        }
    }
    Ok(())
}

fn output_size(project: &Project, options: &ExportOptions, max_dimension: u32) -> Result<(u32, u32)> {
    let (w, h) = (project.canvas.width, project.canvas.height);
    if w < 2 || h < 2 {
        bail!("Canvas dimensions must be at least 2 pixels");
    }
    let short = options.resolution.unwrap_or(w.min(h));
    if !(2..=7680).contains(&short) {
        bail!("Export resolution must be between 2 and 7680");
    }
    let scale = short as f64 / w.min(h) as f64;
    let even = |v: u32| (v as f64 * scale / 2.0).round().max(1.0) * 2.0;
    let (w, h) = (even(w), even(h));
    if w > max_dimension as f64 || h > max_dimension as f64 {
        bail!("This resolution is larger than your GPU supports (max {max_dimension} px)");
    }
    Ok((w as u32, h as u32))
}

/// Start of frame `index`, rounded to the nearest microsecond like cut points made at the
/// playhead, so a clip starting on a frame is the one shown on that frame. Computed from the
/// integer index, never by accumulating rounded durations.
pub fn frame_time_us(index: u64, fps: u32) -> i64 {
    ((2 * index as i128 * 1_000_000 + fps as i128) / (2 * fps as i128)) as i64
}

/// Number of frames that start before `duration_us`.
fn frame_count(duration_us: i64, fps: u32) -> u64 {
    if duration_us <= 0 {
        return 0;
    }
    // frame_time_us(i) < d exactly when i < (2d - 1) * fps / 2_000_000.
    (((2 * duration_us as i128 - 1) * fps as i128 + 1_999_999) / 2_000_000) as u64
}

fn encode(
    project: &Project,
    cache_dir: &Path,
    out: &Path,
    options: &ExportOptions,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(ExportProgress),
    duration: i64,
) -> Result<()> {
    let fps = options.fps.unwrap_or(project.canvas.fps);
    if fps == 0 || fps > 240 {
        bail!("Export frame rate must be between 1 and 240");
    }
    let mut renderer = Renderer::new()?;
    let (w, h) = output_size(project, options, renderer.max_texture_dimension())?;
    let total_frames = frame_count(duration, fps);

    let mut octx = ff::format::output(out).with_context(|| format!("Cannot create {}", out.display()))?;
    let global_header = octx.format().flags().contains(ff::format::Flags::GLOBAL_HEADER);

    let vcodec = ff::encoder::find_by_name("libx264")
        .or_else(|| ff::encoder::find(ff::codec::Id::H264))
        .ok_or_else(|| anyhow!("No H.264 encoder available in this FFmpeg build"))?;
    let mut venc = ff::codec::context::Context::new_with_codec(vcodec).encoder().video()?;
    venc.set_width(w);
    venc.set_height(h);
    venc.set_format(Pixel::YUV420P);
    venc.set_time_base((1, fps as i32));
    venc.set_frame_rate(Some((fps as i32, 1)));
    venc.set_colorspace(color::Space::BT709);
    venc.set_color_range(color::Range::MPEG);
    unsafe {
        let ctx = venc.as_mut_ptr();
        (*ctx).color_primaries = ff::ffi::AVColorPrimaries::AVCOL_PRI_BT709;
        (*ctx).color_trc = ff::ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
    }
    if global_header {
        venc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut vopts = ff::Dictionary::new();
    vopts.set("preset", &options.preset);
    vopts.set("crf", &options.crf.to_string());
    let mut venc = venc.open_with(vopts).context("Opening the H.264 encoder failed")?;
    let vindex = {
        let mut st = octx.add_stream(vcodec)?;
        st.set_parameters(&venc);
        st.set_time_base((1, fps as i32));
        st.index()
    };

    // The native AAC encoder; libfdk_aac is not GPL compatible.
    let acodec = ff::encoder::find_by_name("aac").ok_or_else(|| anyhow!("No AAC encoder available"))?;
    let mut aenc = ff::codec::context::Context::new_with_codec(acodec).encoder().audio()?;
    aenc.set_rate(SAMPLE_RATE as i32);
    aenc.set_channel_layout(ff::ChannelLayout::STEREO);
    aenc.set_format(ff::format::Sample::F32(ff::format::sample::Type::Planar));
    aenc.set_bit_rate(192_000);
    aenc.set_time_base((1, SAMPLE_RATE as i32));
    if global_header {
        aenc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut aenc = aenc.open_with(ff::Dictionary::new()).context("Opening the AAC encoder failed")?;
    let aframe_size = (aenc.frame_size() as usize).max(1024);
    let aindex = {
        let mut st = octx.add_stream(acodec)?;
        st.set_parameters(&aenc);
        st.set_time_base((1, SAMPLE_RATE as i32));
        st.index()
    };

    let mut header_opts = ff::Dictionary::new();
    header_opts.set("movflags", "+faststart");
    octx.write_header_with(header_opts)?;
    let vtb = octx.stream(vindex).unwrap().time_base();
    let atb = octx.stream(aindex).unwrap().time_base();

    let mut mixer = Mixer::new(cache_dir.to_path_buf());
    let mut scaler = scaling::Context::get(Pixel::RGBA, w, h, Pixel::YUV420P, w, h, scaling::Flags::BICUBIC)?;
    set_sws_colorspace(&mut scaler, ff::ffi::SWS_CS_DEFAULT, true, ff::ffi::SWS_CS_ITU709, false);
    let mut rgba = frame::Video::new(Pixel::RGBA, w, h);
    let mut yuv = frame::Video::new(Pixel::YUV420P, w, h);
    let total_samples = crate::audio::us_to_samples(duration);
    let mut audio_pos: i64 = 0;
    let mut mix = vec![0f32; aframe_size * CHANNELS];

    let drain = |enc: &mut ff::encoder::Encoder,
                 index: usize,
                 from: ff::Rational,
                 to: ff::Rational,
                 octx: &mut ff::format::context::Output|
     -> Result<()> {
        let mut packet = ff::Packet::empty();
        while enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(index);
            if index == vindex {
                packet.set_duration(1);
            }
            packet.rescale_ts(from, to);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    };

    for i in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            bail!("CANCELLED: export cancelled");
        }
        let t = frame_time_us(i, fps);
        let pixels = renderer.render(project, t, w, h, Wait::Exact, true)?;
        let stride = rgba.stride(0);
        let row = w as usize * 4;
        let dst = rgba.data_mut(0);
        for y in 0..h as usize {
            dst[y * stride..y * stride + row].copy_from_slice(&pixels[y * row..(y + 1) * row]);
        }
        scaler.run(&rgba, &mut yuv)?;
        yuv.set_pts(Some(i as i64));
        venc.send_frame(&yuv)?;
        drain(&mut venc, vindex, (1, fps as i32).into(), vtb, &mut octx)?;

        // Keep audio up to the end of this video frame.
        let audio_until = crate::audio::us_to_samples(frame_time_us(i + 1, fps)).min(total_samples);
        while audio_pos < audio_until {
            encode_audio(&mut aenc, &mut mixer, project, audio_pos, &mut mix, aframe_size)?;
            audio_pos += aframe_size as i64;
            drain(&mut aenc, aindex, (1, SAMPLE_RATE as i32).into(), atb, &mut octx)?;
        }
        progress(ExportProgress { frame: i + 1, total_frames });
    }

    venc.send_eof()?;
    drain(&mut venc, vindex, (1, fps as i32).into(), vtb, &mut octx)?;
    aenc.send_eof()?;
    drain(&mut aenc, aindex, (1, SAMPLE_RATE as i32).into(), atb, &mut octx)?;
    octx.write_trailer()?;
    Ok(())
}

fn encode_audio(
    enc: &mut ff::encoder::Audio,
    mixer: &mut Mixer,
    project: &Project,
    pos: i64,
    mix: &mut [f32],
    frame_size: usize,
) -> Result<()> {
    mixer.mix(project, pos, mix);
    let mut f = frame::Audio::new(
        ff::format::Sample::F32(ff::format::sample::Type::Planar),
        frame_size,
        ff::ChannelLayout::STEREO,
    );
    f.set_rate(SAMPLE_RATE);
    for ch in 0..CHANNELS {
        let plane = f.plane_mut::<f32>(ch);
        for (n, s) in plane.iter_mut().enumerate().take(frame_size) {
            *s = mix[n * CHANNELS + ch];
        }
    }
    f.set_pts(Some(pos));
    enc.send_frame(&f)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_refuses_late_collision_unless_replace_was_confirmed() {
        let dir = std::env::temp_dir().join(format!("capopen-publish-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("rendered.mp4");
        let out = dir.join("out.mp4");
        let cancel = AtomicBool::new(false);
        std::fs::write(&tmp, b"rendered").unwrap();
        std::fs::write(&out, b"appeared during rendering").unwrap();
        assert!(!ExportOptions::default().replace_existing);
        assert!(publish(&tmp, &out, false, &cancel).unwrap_err().to_string().contains("OUTPUT_EXISTS"));
        assert_eq!(std::fs::read(&out).unwrap(), b"appeared during rendering");
        assert!(tmp.exists());
        publish(&tmp, &out, true, &cancel).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"rendered");
        assert!(!tmp.exists());
        std::fs::remove_file(&out).unwrap();
        std::fs::write(&tmp, b"new render").unwrap();
        publish(&tmp, &out, false, &cancel).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"new render");
        assert!(!tmp.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn publish_without_hard_links_claims_the_name_and_never_replaces() {
        // EPERM, what Linux returns for a hard link on FAT32 and exFAT.
        let no_links = |_: &Path, _: &Path| Err(std::io::Error::from_raw_os_error(1));
        let dir = std::env::temp_dir().join(format!("capopen-publish-fat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("rendered.mp4");
        let out = dir.join("out.mp4");
        let cancel = AtomicBool::new(false);
        std::fs::write(&tmp, b"rendered").unwrap();
        std::fs::write(&out, b"appeared during rendering").unwrap();
        let error = publish_with(&tmp, &out, false, &cancel, no_links).unwrap_err();
        assert!(error.to_string().contains("OUTPUT_EXISTS"), "{error:#}");
        assert_eq!(std::fs::read(&out).unwrap(), b"appeared during rendering");
        assert!(tmp.exists());
        std::fs::remove_file(&out).unwrap();
        publish_with(&tmp, &out, false, &cancel, no_links).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"rendered");
        assert!(!tmp.exists());
        // A failed rename leaves neither the claimed name nor a partial file behind.
        let other = dir.join("other.mp4");
        assert!(publish_with(&tmp, &other, false, &cancel, no_links).is_err());
        assert!(!other.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn publish_honors_cancel_after_rendering_in_both_modes() {
        let dir = std::env::temp_dir().join(format!("capopen-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("rendered.mp4");
        let out = dir.join("out.mp4");
        std::fs::write(&tmp, b"finished render").unwrap();
        let cancel = AtomicBool::new(false);
        cancel.store(true, Ordering::Relaxed);
        for replace in [false, true] {
            assert!(publish(&tmp, &out, replace, &cancel).unwrap_err().to_string().contains("cancelled"));
            assert!(!out.exists());
            std::fs::write(&out, b"original").unwrap();
            assert!(publish(&tmp, &out, replace, &cancel).is_err());
            assert_eq!(std::fs::read(&out).unwrap(), b"original");
            std::fs::remove_file(&out).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn quality_matches_desktop_presets() {
        assert_eq!(Quality::High.crf(), 17);
        assert_eq!(Quality::Recommended.crf(), 21);
        assert_eq!(Quality::Small.crf(), 26);
        assert_eq!(ExportOptions::default().crf, Quality::Recommended.crf());
    }

    #[test]
    fn frames_start_on_the_rounded_times_cuts_are_made_at() {
        // The playhead snaps to round(134 / 30 s) = 4_466_667 µs; the frame must not be 4_466_666.
        assert_eq!(frame_time_us(134, 30), 4_466_667);
        assert_eq!(frame_time_us(1, 30), 33_333);
        assert_eq!(frame_time_us(30, 30), 1_000_000);
        assert_eq!(frame_time_us(1, 24), 41_667);
    }

    #[test]
    fn frame_count_covers_frames_that_start_before_the_end() {
        assert_eq!(frame_count(0, 30), 0);
        assert_eq!(frame_count(1, 30), 1);
        assert_eq!(frame_count(91_900_000, 30), 2757);
        // A timeline ending on a rounded-up frame time has no frame at the end itself.
        assert_eq!(frame_count(4_466_667, 30), 134);
        assert_eq!(frame_count(4_466_668, 30), 135);
        for d in [1, 33_333, 33_334, 1_000_000, 4_466_666, 4_466_667, 91_900_000] {
            let n = frame_count(d, 30);
            assert!(frame_time_us(n - 1, 30) < d && frame_time_us(n, 30) >= d, "{d}");
        }
    }

    #[test]
    fn oversized_portrait_export_returns_error() {
        let project = Project::new("oversized");
        let options = ExportOptions { resolution: Some(7680), ..ExportOptions::default() };
        let result = output_size(&project, &options, 8192);
        assert!(result.is_err(), "7680 x 13654 exceeds the 8192 px device limit");
        assert_eq!(result.unwrap_err().to_string(), "This resolution is larger than your GPU supports (max 8192 px)");
    }

    #[test]
    fn output_resolution_keeps_aspect_and_even_dimensions() {
        let mut project = Project::new("size");
        let options = ExportOptions { resolution: Some(720), ..ExportOptions::default() };
        assert_eq!(output_size(&project, &options, 8192).unwrap(), (720, 1280));
        project.canvas.width = 1920;
        project.canvas.height = 1080;
        assert_eq!(output_size(&project, &options, 8192).unwrap(), (1280, 720));
        project.canvas.width = 1001;
        project.canvas.height = 777;
        let (w, h) = output_size(&project, &options, 8192).unwrap();
        assert_eq!((w % 2, h % 2, h), (0, 0, 720));
        assert!((w as f64 / h as f64 - 1001.0 / 777.0).abs() < 0.002);
    }
}
