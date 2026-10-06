//! MP4 export: the preview renderer at canvas resolution, H.264 + AAC.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use ffmpeg_next as ff;
use ff::software::scaling;
use ff::util::{color, format::Pixel, frame};

use crate::audio::{Mixer, ensure_pcm, has_audio};
use crate::media::{init, set_sws_colorspace};
use crate::model::{CHANNELS, Project, SAMPLE_RATE};
use crate::render::{Renderer, Wait};

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub crf: u8,
    pub preset: String,
    /// Short side of the output in pixels (720, 1080, 1440, 2160); `None` keeps the canvas size.
    pub resolution: Option<u32>,
    /// Output frame rate; `None` uses the project frame rate.
    pub fps: Option<u32>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { crf: 20, preset: "veryfast".into(), resolution: None, fps: None }
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
    if let Ok(target) = std::fs::canonicalize(out) {
        if project.assets.iter().any(|a| std::fs::canonicalize(&a.path).ok().as_ref() == Some(&target)) {
            bail!("{} is used in this project. Choose another file name.", out.display());
        }
    }
    for asset in project.assets.iter().filter(|a| has_audio(a)) {
        ensure_pcm(cache_dir, asset, |_| {})?;
    }
    // Render into a temporary file so a failed export never touches an existing file.
    let tmp = out.with_extension("capopen-part.mp4");
    let result = encode(project, cache_dir, &tmp, options, cancel, &mut progress, duration);
    match result {
        Ok(()) => std::fs::rename(&tmp, out).with_context(|| format!("Cannot write {}", out.display())),
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            Err(e)
        }
    }
}

fn output_size(project: &Project, options: &ExportOptions, max_dimension: u32) -> Result<(u32, u32)> {
    let (w, h) = (project.canvas.width, project.canvas.height);
    if w < 2 || h < 2 { bail!("Canvas dimensions must be at least 2 pixels"); }
    let short = options.resolution.unwrap_or(w.min(h));
    if short < 2 || short > 7680 { bail!("Export resolution must be between 2 and 7680"); }
    let scale = short as f64 / w.min(h) as f64;
    let even = |v: u32| (v as f64 * scale / 2.0).round().max(1.0) * 2.0;
    let (w, h) = (even(w), even(h));
    if w > max_dimension as f64 || h > max_dimension as f64 {
        bail!("This resolution is larger than your GPU supports (max {max_dimension} px)");
    }
    Ok((w as u32, h as u32))
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
    if fps == 0 || fps > 240 { bail!("Export frame rate must be between 1 and 240"); }
    let mut renderer = Renderer::new()?;
    let (w, h) = output_size(project, options, renderer.max_texture_dimension())?;
    let total_frames = ((duration as i128 * fps as i128 + 999_999) / 1_000_000) as u64;

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
    set_sws_colorspace(&mut scaler, ff::ffi::SWS_CS_DEFAULT as i32, true, ff::ffi::SWS_CS_ITU709 as i32, false);
    let mut rgba = frame::Video::new(Pixel::RGBA, w, h);
    let mut yuv = frame::Video::new(Pixel::YUV420P, w, h);
    let total_samples = crate::audio::us_to_samples(duration);
    let mut audio_pos: i64 = 0;
    let mut mix = vec![0f32; aframe_size * CHANNELS];

    let drain = |enc: &mut ff::encoder::Encoder, index: usize, from: ff::Rational, to: ff::Rational, octx: &mut ff::format::context::Output| -> Result<()> {
        let mut packet = ff::Packet::empty();
        while enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(index);
            packet.rescale_ts(from, to);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    };

    for i in 0..total_frames {
        if cancel.load(Ordering::Relaxed) {
            bail!("Export cancelled");
        }
        // Frame time from the integer index, never by accumulating rounded durations.
        let t = (i as i128 * 1_000_000 / fps as i128) as i64;
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
        let audio_until = crate::audio::us_to_samples(((i + 1) as i128 * 1_000_000 / fps as i128) as i64).min(total_samples);
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
    let mut f = frame::Audio::new(ff::format::Sample::F32(ff::format::sample::Type::Planar), frame_size, ff::ChannelLayout::STEREO);
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
