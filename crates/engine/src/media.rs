//! Probing, video decoding and audio extraction on top of FFmpeg.
//!
//! All media times are microseconds relative to the container start, so audio and
//! video of the same file share one origin.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::{Arc, Once};

use anyhow::{Context as _, Result, anyhow, bail};
use ff::codec::packet::side_data::Type as SideDataType;
use ff::format::stream::Disposition;
use ff::software::{resampling, scaling};
use ff::util::{color, format::Pixel, frame};
use ffmpeg_next as ff;

use crate::model::{Asset, AssetKind, CHANNELS, SAMPLE_RATE};

pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        ff::init().expect("FFmpeg failed to initialise");
        ff::util::log::set_level(ff::util::log::Level::Error);
    });
}

fn origin_us(input: &ff::format::context::Input) -> i64 {
    let start = unsafe { (*input.as_ptr()).start_time };
    if start == ff::ffi::AV_NOPTS_VALUE { 0 } else { start }
}

fn is_image_format(input: &ff::format::context::Input) -> bool {
    let format = input.format();
    let name = format.name();
    name == "image2" || name.ends_with("_pipe")
}

fn video_stream(input: &ff::format::context::Input) -> Option<ff::format::stream::Stream<'_>> {
    input
        .streams()
        .filter(|s| s.parameters().medium() == ff::media::Type::Video)
        .find(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
}

/// Clockwise rotation in degrees from the display matrix, snapped to 0/90/180/270.
fn display_rotation(stream: &ff::format::stream::Stream) -> u32 {
    for sd in stream.side_data() {
        if sd.kind() == SideDataType::DisplayMatrix && sd.data().len() >= 36 {
            let ccw = unsafe { ff::ffi::av_display_rotation_get(sd.data().as_ptr() as *const i32) };
            if ccw.is_nan() {
                return 0;
            }
            let cw = (-ccw).rem_euclid(360.0);
            return ((cw / 90.0).round() as u32 % 4) * 90;
        }
    }
    0
}

pub fn probe(path: &Path, id: String) -> Result<Asset> {
    init();
    let input = ff::format::input(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let is_image = is_image_format(&input);
    let video = video_stream(&input);
    let audio = input.streams().best(ff::media::Type::Audio);
    if video.is_none() && audio.is_none() {
        bail!("{} has no video or audio", path.display());
    }

    let mut asset = Asset {
        id,
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned(),
        kind: if is_image {
            AssetKind::Image
        } else if video.is_some() {
            AssetKind::Video
        } else {
            AssetKind::Audio
        },
        duration_us: if is_image { 0 } else { input.duration().max(0) },
        width: 0,
        height: 0,
        fps: 0.0,
        has_audio: audio.is_some() && !is_image,
        rotation: 0,
    };

    if let Some(stream) = video {
        let decoder = ff::codec::context::Context::from_parameters(stream.parameters())?.decoder().video()?;
        let rotation = display_rotation(&stream);
        let (w, h) = (decoder.width(), decoder.height());
        (asset.width, asset.height) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
        asset.rotation = rotation;
        let rate = stream.avg_frame_rate();
        let rate = if rate.denominator() == 0 || rate.numerator() == 0 { stream.rate() } else { rate };
        asset.fps = if rate.denominator() == 0 { 0.0 } else { f64::from(rate) };
    }
    Ok(asset)
}

/// One decoded frame converted to tightly packed RGBA, in source orientation.
#[derive(Clone)]
pub struct RgbaFrame {
    pub t_us: i64,
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

struct Scaler {
    key: (Pixel, u32, u32, u32, u32),
    ctx: scaling::Context,
}

// SwsContext is only touched by the thread that owns the decoder.
unsafe impl Send for Scaler {}

fn sws_colorspace(space: color::Space, height: u32) -> i32 {
    use ff::ffi::*;
    match space {
        color::Space::BT709 => SWS_CS_ITU709,
        color::Space::BT2020NCL | color::Space::BT2020CL => SWS_CS_BT2020,
        color::Space::SMPTE240M => SWS_CS_SMPTE240M,
        color::Space::FCC => SWS_CS_FCC,
        color::Space::BT470BG | color::Space::SMPTE170M => SWS_CS_ITU601,
        _ if height >= 720 => SWS_CS_ITU709,
        _ => SWS_CS_ITU601,
    }
}

/// Sets the YUV matrix and range explicitly; swscale would otherwise assume BT.601.
pub(crate) fn set_sws_colorspace(ctx: &mut scaling::Context, src_cs: i32, src_full: bool, dst_cs: i32, dst_full: bool) {
    use ff::ffi::*;
    unsafe {
        let ptr = ctx.as_mut_ptr();
        let (mut inv, mut table) = (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut sr, mut dr, mut b, mut c, mut s) = (0, 0, 0, 0, 0);
        if sws_getColorspaceDetails(ptr, &mut inv, &mut sr, &mut table, &mut dr, &mut b, &mut c, &mut s) < 0 {
            return;
        }
        sws_setColorspaceDetails(
            ptr,
            sws_getCoefficients(src_cs),
            src_full as i32,
            sws_getCoefficients(dst_cs),
            dst_full as i32,
            b,
            c,
            s,
        );
    }
}

fn is_full_range(f: &frame::Video) -> bool {
    f.color_range() == color::Range::JPEG
        || matches!(f.format(), Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P | Pixel::YUVJ440P)
}

fn to_rgba(scaler: &mut Option<Scaler>, f: &frame::Video, t_us: i64, w: u32, h: u32) -> Result<RgbaFrame> {
    let key = (f.format(), f.width(), f.height(), w, h);
    if scaler.as_ref().map(|s| s.key) != Some(key) {
        let mut ctx =
            scaling::Context::get(f.format(), f.width(), f.height(), Pixel::RGBA, w, h, scaling::Flags::BILINEAR)?;
        let src_cs = sws_colorspace(f.color_space(), f.height());
        set_sws_colorspace(&mut ctx, src_cs, is_full_range(f), ff::ffi::SWS_CS_DEFAULT, true);
        *scaler = Some(Scaler { key, ctx });
    }
    let mut out = frame::Video::new(Pixel::RGBA, w, h);
    scaler.as_mut().unwrap().ctx.run(f, &mut out)?;
    let stride = out.stride(0);
    let row = w as usize * 4;
    let src = out.data(0);
    let mut data = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        data.extend_from_slice(&src[y * stride..y * stride + row]);
    }
    Ok(RgbaFrame { t_us, width: w, height: h, data: Arc::new(data) })
}

pub type DecodedFrame = (i64, frame::Video);

/// Sequential decoder with exact seeking. Picks the last frame whose real timestamp is
/// at or before the requested time, so variable frame rate footage stays exact.
pub struct VideoDecoder {
    input: ff::format::context::Input,
    stream_index: usize,
    decoder: ff::decoder::Video,
    time_base: ff::Rational,
    origin_us: i64,
    sent_eof: bool,
    eof: bool,
    is_image: bool,
    /// Last decoded frame and its time; frames are returned in display order.
    scaler: Option<Scaler>,
    pub rotation: u32,
    pub frame_duration_us: i64,
}

unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        init();
        let input = ff::format::input(path).with_context(|| format!("Cannot open {}", path.display()))?;
        let is_image = is_image_format(&input);
        let stream = video_stream(&input).ok_or_else(|| anyhow!("{} has no video", path.display()))?;
        let stream_index = stream.index();
        let rotation = display_rotation(&stream);
        let time_base = stream.time_base();
        let rate = stream.avg_frame_rate();
        let fps = if rate.numerator() > 0 && rate.denominator() > 0 { f64::from(rate) } else { 30.0 };
        let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
        ctx.set_threading(ff::codec::threading::Config { kind: ff::codec::threading::Type::Frame, count: 4 });
        let decoder = ctx.decoder().video()?;
        let origin_us = origin_us(&input);
        Ok(Self {
            input,
            stream_index,
            decoder,
            time_base,
            origin_us,
            sent_eof: false,
            eof: false,
            is_image,
            scaler: None,
            rotation,
            frame_duration_us: (1_000_000.0 / fps.clamp(1.0, 240.0)) as i64,
        })
    }

    pub fn is_image(&self) -> bool {
        self.is_image
    }

    pub fn source_size(&self) -> (u32, u32) {
        (self.decoder.width(), self.decoder.height())
    }

    pub fn is_eof(&self) -> bool {
        self.eof
    }

    /// Jumps to the keyframe at or before `t_us`. The next decoded frames start there.
    pub fn seek(&mut self, t_us: i64) -> Result<()> {
        use ff::util::mathematics::{Rescale, Rounding};
        let ts =
            t_us.max(0).saturating_add(self.origin_us).rescale_with((1, 1_000_000), self.time_base, Rounding::Down);
        // A global-time seek rounds to the nearest stream tick and can skip the covering GOP.
        let result = unsafe {
            ff::ffi::avformat_seek_file(self.input.as_mut_ptr(), self.stream_index as i32, i64::MIN, ts, ts, 0)
        };
        // Seeking images and tiny files can fail harmlessly; decoding restarts from the start.
        if result < 0 {
            self.input.seek(0, ..).ok();
        }
        self.decoder.flush();
        self.sent_eof = false;
        self.eof = false;
        Ok(())
    }

    /// Decodes the next frame in display order. `None` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<(i64, frame::Video)>> {
        if self.eof {
            return Ok(None);
        }
        let mut f = frame::Video::empty();
        loop {
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => {
                    let pts = f.timestamp().or(f.pts()).unwrap_or(0);
                    let t = (pts as f64 * f64::from(self.time_base) * 1e6).round() as i64 - self.origin_us;
                    return Ok(Some((t, f)));
                }
                Err(ff::Error::Eof) => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => {}
                Err(e) => return Err(e.into()),
            }
            if self.sent_eof {
                self.eof = true;
                return Ok(None);
            }
            loop {
                match self.input.packets().next() {
                    Some((stream, packet)) if stream.index() == self.stream_index => {
                        if let Err(e) = self.decoder.send_packet(&packet) {
                            log::warn!("Skipping corrupt packet: {e}");
                        }
                        break;
                    }
                    Some(_) => continue,
                    None => {
                        self.decoder.send_eof().ok();
                        self.sent_eof = true;
                        break;
                    }
                }
            }
        }
    }

    /// Returns the frame covering `want` and the next frame, if decoded.
    /// Pass the previous pair when decoding forward; after seeking pass `[None, None]`.
    pub fn frame_covering(
        &mut self,
        want: i64,
        [mut covering, mut next]: [Option<DecodedFrame>; 2],
    ) -> Result<[Option<DecodedFrame>; 2]> {
        loop {
            let decoded = match next.take() {
                Some(frame) => Some(frame),
                None => self.next_frame()?,
            };
            match decoded {
                Some((t, f)) if t == want => return Ok([Some((t, f)), None]),
                Some((t, f)) if t < want || covering.is_none() && self.is_image() => covering = Some((t, f)),
                next => return Ok([covering, next]),
            }
        }
    }

    pub fn convert(&mut self, f: &frame::Video, t_us: i64, w: u32, h: u32) -> Result<RgbaFrame> {
        to_rgba(&mut self.scaler, f, t_us, w.max(2), h.max(2))
    }
}

/// Size to decode at so a layer shown at `display` pixels (after rotation) stays sharp
/// without converting more pixels than needed. Returned in source orientation.
pub fn decode_size(src: (u32, u32), rotation: u32, display: (f32, f32)) -> (u32, u32) {
    let (dw, dh) = if rotation % 180 == 90 { (display.1, display.0) } else { display };
    let scale = (dw / src.0 as f32).max(dh / src.1 as f32).min(1.0);
    let even = |v: f32| ((v.round() as u32).max(2) + 1) & !1;
    (even(src.0 as f32 * scale), even(src.1 as f32 * scale))
}

/// Decodes the whole audio stream to 48 kHz interleaved stereo f32 little-endian.
/// The file starts at the container origin, padded with silence if audio starts late.
pub fn extract_pcm(path: &Path, out: &Path, mut progress: impl FnMut(f32)) -> Result<u64> {
    init();
    let mut input = ff::format::input(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let stream = input.streams().best(ff::media::Type::Audio).ok_or_else(|| anyhow!("No audio stream"))?;
    let stream_index = stream.index();
    let time_base = f64::from(stream.time_base());
    let video = input.streams().best(ff::media::Type::Video).map(|s| (s.index(), f64::from(s.time_base())));
    let duration_us = input.duration().max(1) as f64;
    let origin = origin_us(&input);
    let mut decoder = ff::codec::context::Context::from_parameters(stream.parameters())?.decoder().audio()?;

    let tmp = out.with_file_name(format!(".capopen-pcm-{}.part", crate::edit::new_id()));
    let file = File::options().write(true).create_new(true).open(&tmp)?;
    let result = (|| {
        let mut writer = BufWriter::with_capacity(1 << 20, file);
        let mut written: u64 = 0; // sample frames
        let mut resampler: Option<(resampling::Context, (ff::format::Sample, u64, u32))> = None;
        let mut decoded = frame::Audio::empty();
        let mut last_progress = 0.0;
        let mut mono = false;

        // Writes resampled audio as stereo. Mono sources are resampled as mono and duplicated,
        // so they keep their level instead of FFmpeg's -3 dB pan.
        let write_out =
            |out: &frame::Audio, mono: bool, writer: &mut BufWriter<File>, written: &mut u64| -> Result<()> {
                let n = out.samples();
                if n == 0 {
                    return Ok(());
                }
                if mono {
                    let src: &[f32] = &bytemuck_slice(out.data(0))[..n];
                    let mut buf = Vec::with_capacity(n * CHANNELS * 4);
                    for s in src {
                        buf.extend_from_slice(&s.to_le_bytes());
                        buf.extend_from_slice(&s.to_le_bytes());
                    }
                    writer.write_all(&buf)?;
                } else {
                    writer.write_all(&out.data(0)[..n * CHANNELS * 4])?;
                }
                *written += n as u64;
                Ok(())
            };

        let write_frame = |f: &frame::Audio,
                           resampler: &mut Option<(resampling::Context, (ff::format::Sample, u64, u32))>,
                           mono: &mut bool,
                           writer: &mut BufWriter<File>,
                           written: &mut u64,
                           max_lead_us: i64|
         -> Result<()> {
            let mut layout = f.channel_layout();
            if layout.is_empty() {
                layout = ff::ChannelLayout::default(f.channels() as i32);
            }
            let key = (f.format(), layout.bits(), f.rate());
            if resampler.as_ref().map(|r| r.1) != Some(key) {
                *mono = layout.channels() == 1;
                let dst = if *mono { ff::ChannelLayout::MONO } else { ff::ChannelLayout::STEREO };
                let ctx = resampling::Context::get(f.format(), layout, f.rate(), PCM_FORMAT, dst, SAMPLE_RATE)?;
                *resampler = Some((ctx, key));
            }
            // Align the first samples with the container origin.
            if *written == 0
                && let Some(pts) = f.timestamp().or(f.pts())
            {
                let start_us = ((pts as f64 * time_base * 1e6) as i64).saturating_sub(origin).max(0);
                // The container duration includes a broken offset, so it cannot bound the silence.
                if start_us > max_lead_us {
                    bail!("Audio starts {} s after the video; the file's timestamps look broken", start_us / 1_000_000);
                }
                let pad = (start_us as u64 * SAMPLE_RATE as u64) / 1_000_000;
                std::io::copy(&mut std::io::repeat(0).take(pad * (CHANNELS * 4) as u64), writer)?;
                *written += pad;
            }
            let mut f = f.clone();
            f.set_channel_layout(layout);
            let (ctx, _) = resampler.as_mut().unwrap();
            // ffmpeg-next sizes the output like the input, which drops samples when upsampling
            // (22.05 or 44.1 kHz to 48 kHz). Allocate for the converted length instead.
            let capacity = f.samples() * SAMPLE_RATE as usize / f.rate().max(1) as usize + 256;
            let mut out = frame::Audio::new(PCM_FORMAT, capacity, ctx.output().channel_layout);
            ctx.run(&f, &mut out)?;
            write_out(&out, *mono, writer, written)
        };

        // Video read so far shows how late real audio may start; demuxing interleaves by time.
        let mut video_end_us = 0;
        for (s, packet) in input.packets() {
            if s.index() != stream_index {
                if let (Some((index, base)), Some(pts)) = (video, packet.pts())
                    && s.index() == index
                {
                    let end_us = (pts.saturating_add(packet.duration()) as f64 * base * 1e6) as i64;
                    video_end_us = video_end_us.max(end_us.saturating_sub(origin));
                }
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            while decoder.receive_frame(&mut decoded).is_ok() {
                write_frame(
                    &decoded,
                    &mut resampler,
                    &mut mono,
                    &mut writer,
                    &mut written,
                    video_end_us.max(MAX_AUDIO_LEAD_US),
                )?;
            }
            if let Some(pts) = packet.pts() {
                let p = (pts as f64 * time_base * 1e6 / duration_us) as f32;
                if p - last_progress > 0.02 {
                    last_progress = p;
                    progress(p.clamp(0.0, 1.0));
                }
            }
        }
        decoder.send_eof().ok();
        while decoder.receive_frame(&mut decoded).is_ok() {
            write_frame(
                &decoded,
                &mut resampler,
                &mut mono,
                &mut writer,
                &mut written,
                video_end_us.max(MAX_AUDIO_LEAD_US),
            )?;
        }
        if let Some((ctx, _)) = resampler.as_mut() {
            loop {
                let mut tail = frame::Audio::new(PCM_FORMAT, 4096, ctx.output().channel_layout);
                if ctx.flush(&mut tail).is_err() || tail.samples() == 0 {
                    break;
                }
                write_out(&tail, mono, &mut writer, &mut written)?;
            }
        }
        writer.flush()?;
        drop(writer);
        std::fs::rename(&tmp, out)?;
        progress(1.0);
        Ok(written)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Audio may start this late even before the video read so far covers it; later starts past the
/// video are broken timestamps.
const MAX_AUDIO_LEAD_US: i64 = 600_000_000;

const PCM_FORMAT: ff::format::Sample = ff::format::Sample::F32(ff::format::sample::Type::Packed);

fn bytemuck_slice(bytes: &[u8]) -> &[f32] {
    bytemuck::cast_slice(&bytes[..bytes.len() / 4 * 4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_before_keyframe_respects_container_start_offset() {
        let path = std::env::temp_dir().join(format!("capopen-seek-{}.mp4", uuid::Uuid::new_v4()));
        let encoded = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=96x64:r=25:d=1",
                "-c:v",
                "libx264",
                "-threads",
                "2",
                "-g",
                "10",
                "-bf",
                "3",
                "-output_ts_offset",
                "5",
            ])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !encoded {
            eprintln!("ffmpeg CLI with libx264 not available, skipping");
            return;
        }
        let mut decoder = VideoDecoder::open(&path).unwrap();
        assert_eq!(decoder.origin_us, 5_000_000);
        decoder.seek(399_999).unwrap();
        let [covering, next] = decoder.frame_covering(399_999, [None, None]).unwrap();
        assert_eq!(covering.unwrap().0, 360_000);
        assert_eq!(next.unwrap().0, 400_000);
        drop(decoder);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires the local tmp-test media fixtures"]
    fn covering_frames_match_sequential_decode() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test");
        for name in ["wide.mp4", "phone_hevc_vfr.mov"] {
            let mut reference = VideoDecoder::open(&root.join(name)).unwrap();
            let mut times = Vec::new();
            while let Some((t, _)) = reference.next_frame().unwrap() {
                times.push(t);
            }
            let mut decoder = VideoDecoder::open(&root.join(name)).unwrap();
            let mut pair = [None, None];
            for target in [0, 1, 33_333, 500_001, 900_005, 1_000_000, 2_333_333, 10_000_000] {
                pair = decoder.frame_covering(target, pair).unwrap();
                let expected = times.iter().copied().rfind(|t| *t <= target);
                assert_eq!(pair[0].as_ref().map(|f| f.0), expected, "{name} at {target}");
                decoder.seek(target).unwrap();
                pair = decoder.frame_covering(target, [None, None]).unwrap();
                assert_eq!(pair[0].as_ref().map(|f| f.0), expected, "seek {name} at {target}");
            }
        }
    }

    #[test]
    fn decode_size_keeps_aspect_and_never_upscales() {
        assert_eq!(decode_size((1920, 1080), 0, (960.0, 540.0)), (960, 540));
        assert_eq!(decode_size((1920, 1080), 0, (4000.0, 3000.0)), (1920, 1080));
        // Rotated portrait source shown 540 wide, 960 tall.
        assert_eq!(decode_size((1920, 1080), 90, (540.0, 960.0)), (960, 540));
    }

    /// Upsampled sources used to lose their tail; mono sources came out 3 dB quieter.
    #[test]
    fn pcm_keeps_full_length_and_level_when_upsampling() {
        let dir = std::env::temp_dir().join(format!("capopen-pcm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (rate, layout) in [(22_050, "mono"), (44_100, "stereo")] {
            let src = dir.join(format!("tone_{rate}.wav"));
            let ok = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("sine=frequency=440:sample_rate={rate}:duration=3"))
                .args(["-af", &format!("aformat=channel_layouts={layout}"), "-ar", &rate.to_string()])
                .arg(&src)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                eprintln!("ffmpeg CLI not available, skipping");
                return;
            }
            let out = dir.join(format!("tone_{rate}.f32"));
            let frames = extract_pcm(&src, &out, |_| {}).unwrap();
            assert!((frames as i64 - 3 * SAMPLE_RATE as i64).abs() < 2_000, "{rate} Hz gave {frames} frames");
            let bytes = std::fs::read(&out).unwrap();
            let samples: &[f32] = bytemuck::cast_slice(&bytes);
            let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
            // FFmpeg's sine source peaks at 1/8 of full scale; its own stereo upmix is -3 dB.
            let expected = if layout == "mono" { 0.125 } else { 0.125 * std::f32::consts::FRAC_1_SQRT_2 };
            assert!((peak - expected).abs() < 0.01, "{rate} Hz {layout} peak {peak}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
