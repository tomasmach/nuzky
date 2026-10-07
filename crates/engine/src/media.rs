//! Probing, video decoding and audio extraction on top of FFmpeg.
//!
//! All media times are microseconds relative to the container start, so audio and
//! video of the same file share one origin.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::{Arc, Once, OnceLock};

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

/// Demuxers that open other files or URLs named inside the media, or run scripts.
const INDIRECT_DEMUXERS: &[&str] =
    &["hls", "dash", "concat", "imf", "sdp", "rtsp", "rtp", "sap", "avisynth", "vapoursynth"];

/// Every demuxer except the indirect ones; FFmpeg checks it before reading the file header.
fn format_whitelist() -> &'static str {
    static LIST: OnceLock<String> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut names = Vec::new();
        let mut opaque = std::ptr::null_mut();
        loop {
            let format = unsafe { ff::ffi::av_demuxer_iterate(&mut opaque) };
            if format.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*format).name) }.to_string_lossy();
            if !name.split(',').any(|n| INDIRECT_DEMUXERS.contains(&n)) {
                names.push(name.into_owned());
            }
        }
        names.join(",")
    })
}

/// Opens local media only: no network protocols, also for files a container refers to,
/// and no playlists or scripts that pull in other sources.
fn open_input(path: &Path) -> Result<ff::format::context::Input> {
    init();
    let mut options = ff::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", format_whitelist());
    ff::format::input_with_dictionary(path, options).with_context(|| format!("Cannot open {}", path.display()))
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

/// Clockwise rotation in degrees from a display matrix, snapped to 0/90/180/270.
fn matrix_rotation(matrix: &[u8]) -> Option<u32> {
    if matrix.len() < 36 {
        return None;
    }
    let ccw = unsafe { ff::ffi::av_display_rotation_get(matrix.as_ptr() as *const i32) };
    if ccw.is_nan() {
        return Some(0);
    }
    let cw = (-ccw).rem_euclid(360.0);
    Some(((cw / 90.0).round() as u32 % 4) * 90)
}

fn display_rotation(stream: &ff::format::stream::Stream) -> u32 {
    stream
        .side_data()
        .filter(|sd| sd.kind() == SideDataType::DisplayMatrix)
        .find_map(|sd| matrix_rotation(sd.data()))
        .unwrap_or(0)
}

/// Still images carry EXIF orientation only on the decoded frame.
fn frame_rotation(f: &frame::Video) -> Option<u32> {
    matrix_rotation(f.side_data(frame::side_data::Type::DisplayMatrix)?.data())
}

/// Decodes the first frame of a still image to read its EXIF orientation.
fn image_rotation(input: &mut ff::format::context::Input, index: usize, mut decoder: ff::decoder::Video) -> u32 {
    let mut f = frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() == index && decoder.send_packet(&packet).is_ok() && decoder.receive_frame(&mut f).is_ok() {
            return frame_rotation(&f).unwrap_or(0);
        }
    }
    decoder.send_eof().ok();
    if decoder.receive_frame(&mut f).is_ok() { frame_rotation(&f).unwrap_or(0) } else { 0 }
}

pub fn probe(path: &Path, id: String) -> Result<Asset> {
    let mut input = open_input(path)?;
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
        let mut rotation = display_rotation(&stream);
        let (w, h) = (decoder.width(), decoder.height());
        let rate = stream.avg_frame_rate();
        let rate = if rate.denominator() == 0 || rate.numerator() == 0 { stream.rate() } else { rate };
        asset.fps = if rate.denominator() == 0 { 0.0 } else { f64::from(rate) };
        if is_image && rotation == 0 {
            let index = stream.index();
            rotation = image_rotation(&mut input, index, decoder);
        }
        (asset.width, asset.height) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
        asset.rotation = rotation;
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
    /// The only frame of a still image, replayed after seeking: image2 loses it when seeked.
    image: Option<DecodedFrame>,
    image_rewound: bool,
    scaler: Option<Scaler>,
    pub rotation: u32,
    pub frame_duration_us: i64,
}

unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let input = open_input(path)?;
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
            image: None,
            image_rewound: false,
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
        if self.is_image {
            self.image_rewound = self.image.is_some();
            return Ok(());
        }
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
        if std::mem::take(&mut self.image_rewound)
            && let Some((t, f)) = &self.image
        {
            return Ok(Some((*t, f.clone())));
        }
        if self.eof {
            return Ok(None);
        }
        let mut f = frame::Video::empty();
        loop {
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => {
                    if self.is_image
                        && let Some(rotation) = frame_rotation(&f)
                    {
                        self.rotation = rotation;
                    }
                    let pts = f.timestamp().or(f.pts()).unwrap_or(0);
                    let t = (pts as f64 * f64::from(self.time_base) * 1e6).round() as i64 - self.origin_us;
                    if self.is_image {
                        self.image = Some((t, f.clone()));
                    }
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
/// without converting more pixels than needed, and no side exceeds `max_side`.
/// Returned in source orientation.
pub fn decode_size(src: (u32, u32), rotation: u32, display: (f32, f32), max_side: u32) -> (u32, u32) {
    let (dw, dh) = if rotation % 180 == 90 { (display.1, display.0) } else { display };
    let scale = (dw / src.0 as f32).max(dh / src.1 as f32).min(1.0).min(max_side as f32 / src.0.max(src.1) as f32);
    let even = |v: f32| (((v.round() as u32).max(2) + 1) & !1).min(max_side & !1);
    (even(src.0 as f32 * scale), even(src.1 as f32 * scale))
}

/// Decodes the whole audio stream to 48 kHz interleaved stereo f32 little-endian.
/// The file starts at the container origin, padded with silence if audio starts late.
pub fn extract_pcm(path: &Path, out: &Path, mut progress: impl FnMut(f32)) -> Result<u64> {
    let mut input = open_input(path)?;
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
        let mut sink = PcmSink {
            writer: BufWriter::with_capacity(1 << 20, file),
            written: 0,
            resampler: None,
            mono: false,
            shift: None,
            skip: 0,
        };
        let mut decoded = frame::Audio::empty();
        let mut last_progress = 0.0;
        let start_us = |f: &frame::Audio| {
            f.timestamp().or(f.pts()).map(|pts| ((pts as f64 * time_base * 1e6) as i64).saturating_sub(origin))
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
                sink.write_frame(&decoded, start_us(&decoded), max_lead_us(video_end_us))?;
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
            sink.write_frame(&decoded, start_us(&decoded), max_lead_us(video_end_us))?;
        }
        if let Some((mut ctx, _)) = sink.resampler.take() {
            loop {
                let mut tail = frame::Audio::new(PCM_FORMAT, 4096, ctx.output().channel_layout);
                if ctx.flush(&mut tail).is_err() || tail.samples() == 0 {
                    break;
                }
                sink.write_out(&tail)?;
            }
        }
        let PcmSink { mut writer, written, .. } = sink;
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

/// Writes decoded audio as 48 kHz interleaved stereo f32, placed by its timestamps.
struct PcmSink {
    writer: BufWriter<File>,
    /// Sample frames written so far.
    written: u64,
    resampler: Option<(resampling::Context, (ff::format::Sample, u64, u32))>,
    mono: bool,
    /// Sample frames added to frame times so they land at their timestamps; set by the first timed frame
    /// and moved by timestamp jumps too large to be real.
    shift: Option<i64>,
    /// Converted sample frames still to drop because they overlap audio already written.
    skip: u64,
}

fn us_to_frames(us: i64) -> i64 {
    (us as i128 * SAMPLE_RATE as i128 / 1_000_000) as i64
}

impl PcmSink {
    /// Writes resampled audio as stereo. Mono sources are resampled as mono and duplicated,
    /// so they keep their level instead of FFmpeg's -3 dB pan.
    fn write_out(&mut self, out: &frame::Audio) -> Result<()> {
        let n = out.samples();
        let dropped = self.skip.min(n as u64) as usize;
        self.skip -= dropped as u64;
        if n == dropped {
            return Ok(());
        }
        if self.mono {
            let src: &[f32] = &bytemuck_slice(out.data(0))[dropped..n];
            let mut buf = Vec::with_capacity(src.len() * CHANNELS * 4);
            for s in src {
                buf.extend_from_slice(&s.to_le_bytes());
                buf.extend_from_slice(&s.to_le_bytes());
            }
            self.writer.write_all(&buf)?;
        } else {
            self.writer.write_all(&out.data(0)[dropped * CHANNELS * 4..n * CHANNELS * 4])?;
        }
        self.written += (n - dropped) as u64;
        Ok(())
    }

    fn silence(&mut self, frames: i64) -> Result<()> {
        let frames = frames.max(0) as u64;
        std::io::copy(&mut std::io::repeat(0).take(frames * (CHANNELS * 4) as u64), &mut self.writer)?;
        self.written += frames;
        Ok(())
    }

    /// Lines up the next samples with `start_us`: the first ones with the container origin, later ones
    /// by filling gaps with silence and dropping overlaps, like FFmpeg's aresample async mode.
    fn place(&mut self, start_us: i64, max_lead_us: i64) -> Result<()> {
        let buffered = self.resampler.as_ref().and_then(|(ctx, _)| ctx.delay()).map_or(0, |d| d.output);
        let position = self.written as i64 + buffered - self.skip as i64;
        let Some(shift) = self.shift else {
            // The container duration includes a broken offset, so it cannot bound the silence.
            if start_us > max_lead_us {
                bail!("Audio starts {} s after the video; the file's timestamps look broken", start_us / 1_000_000);
            }
            let target = us_to_frames(start_us).max(position);
            self.shift = Some(target - us_to_frames(start_us));
            return self.silence(target - position);
        };
        let target = us_to_frames(start_us) + shift;
        let gap = target - position;
        if gap.abs() <= us_to_frames(MIN_GAP_US) {
            return Ok(());
        }
        // Silence is bounded like the lead-in, so broken timestamps cannot fill the disk.
        if gap > 0 && target * 1_000_000 / SAMPLE_RATE as i64 <= max_lead_us {
            return self.silence(gap);
        }
        if gap < 0 && -gap <= us_to_frames(MAX_OVERLAP_US) {
            self.skip += (-gap) as u64;
            return Ok(());
        }
        log::warn!("Ignoring an audio timestamp jump of {} ms", gap * 1000 / SAMPLE_RATE as i64);
        self.shift = Some(shift - gap);
        Ok(())
    }

    fn write_frame(&mut self, f: &frame::Audio, start_us: Option<i64>, max_lead_us: i64) -> Result<()> {
        let mut layout = f.channel_layout();
        if layout.is_empty() {
            layout = ff::ChannelLayout::default(f.channels() as i32);
        }
        let key = (f.format(), layout.bits(), f.rate());
        if self.resampler.as_ref().map(|r| r.1) != Some(key) {
            self.mono = layout.channels() == 1;
            let dst = if self.mono { ff::ChannelLayout::MONO } else { ff::ChannelLayout::STEREO };
            let ctx = resampling::Context::get(f.format(), layout, f.rate(), PCM_FORMAT, dst, SAMPLE_RATE)?;
            self.resampler = Some((ctx, key));
        }
        if let Some(start_us) = start_us {
            self.place(start_us, max_lead_us)?;
        }
        let mut f = f.clone();
        f.set_channel_layout(layout);
        let (ctx, _) = self.resampler.as_mut().unwrap();
        // ffmpeg-next sizes the output like the input, which drops samples when upsampling
        // (22.05 or 44.1 kHz to 48 kHz). Allocate for the converted length instead.
        let capacity = f.samples() * SAMPLE_RATE as usize / f.rate().max(1) as usize + 256;
        let mut out = frame::Audio::new(PCM_FORMAT, capacity, ctx.output().channel_layout);
        ctx.run(&f, &mut out)?;
        self.write_out(&out)
    }
}

/// Timestamp differences up to this are jitter, not gaps (FFmpeg's aresample min_hard_comp).
const MIN_GAP_US: i64 = 100_000;
/// Audio stepping back further than this restarted its timestamps rather than overlapping.
const MAX_OVERLAP_US: i64 = 1_000_000;

/// Audio may start this late even before the video read so far covers it; later starts past the
/// video are broken timestamps.
const MAX_AUDIO_LEAD_US: i64 = 600_000_000;
/// Muxers keep streams this close together in the file (FFmpeg's default max_interleave_delta).
const MAX_INTERLEAVE_US: i64 = 10_000_000;

fn max_lead_us(video_end_us: i64) -> i64 {
    video_end_us.saturating_add(MAX_INTERLEAVE_US).max(MAX_AUDIO_LEAD_US)
}

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

    /// FFmpeg 8 image2 returns no packets once seeked, so a .jpg never reached the preview.
    #[test]
    fn still_jpeg_decodes_after_every_seek() {
        let path = std::env::temp_dir().join(format!("capopen-still-{}.jpg", uuid::Uuid::new_v4()));
        let encoded = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=s=96x64", "-frames:v", "1"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !encoded {
            eprintln!("ffmpeg CLI not available, skipping");
            return;
        }
        let mut decoder = VideoDecoder::open(&path).unwrap();
        for _ in 0..3 {
            decoder.seek(0).unwrap();
            let [covering, next] = decoder.frame_covering(0, [None, None]).unwrap();
            assert_eq!(covering.map(|(t, f)| (t, f.width(), f.height())), Some((0, 96, 64)));
            assert!(next.is_none());
        }
        drop(decoder);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn decode_size_keeps_aspect_and_never_upscales() {
        assert_eq!(decode_size((1920, 1080), 0, (960.0, 540.0), 8192), (960, 540));
        assert_eq!(decode_size((1920, 1080), 0, (4000.0, 3000.0), 8192), (1920, 1080));
        // Rotated portrait source shown 540 wide, 960 tall.
        assert_eq!(decode_size((1920, 1080), 90, (540.0, 960.0), 8192), (960, 540));
        // A zoomed panorama stays inside the GPU texture limit and keeps its aspect.
        assert_eq!(decode_size((20000, 3000), 0, (12800.0, 1920.0), 8192), (8192, 1230));
        assert_eq!(decode_size((3000, 20000), 90, (12800.0, 1920.0), 8191), (1230, 8190));
    }

    /// Recorders that drop audio leave timestamp gaps; joined files can step back and overlap.
    #[test]
    fn pcm_places_samples_by_timestamp() {
        let dir = std::env::temp_dir().join(format!("capopen-pcm-gap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ffmpeg = |args: &[&str], out: &Path| {
            std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=red:s=64x64:r=25", "-f", "lavfi", "-i"])
                .args(args)
                .args(["-c:v", "libx264", "-threads", "2", "-c:a", "aac"])
                .arg(out)
                .status()
                .is_ok_and(|s| s.success())
        };
        let (gap, a, b) = (dir.join("gap.mkv"), dir.join("a.ts"), dir.join("b.ts"));
        // A 1 s tone, 1 s without audio packets, another 1 s tone.
        if !ffmpeg(&["sine=f=440:r=48000:d=2,asetpts='PTS+if(gte(T,1),1/TB,0)'", "-t", "3"], &gap)
            || !ffmpeg(&["sine=f=440:r=48000:d=1", "-t", "1"], &a)
            || !ffmpeg(&["sine=f=880:r=48000:d=1", "-t", "1", "-output_ts_offset", "0.7"], &b)
        {
            eprintln!("ffmpeg CLI with libx264 not available, skipping");
            return;
        }
        let pcm = |path: &Path| {
            let out = path.with_extension("f32");
            extract_pcm(path, &out, |_| {}).unwrap();
            let samples: Vec<f32> = bytemuck::cast_slice(&std::fs::read(&out).unwrap()).to_vec();
            // Peak level between two times in seconds.
            move |from: f64, to: f64| {
                let end = ((to * 96_000.0) as usize).min(samples.len());
                samples[((from * 96_000.0) as usize).min(end)..end].iter().fold(0f32, |m, s| m.max(s.abs()))
            }
        };
        let peak = pcm(&gap);
        assert!(peak(0.1, 0.9) > 0.05 && peak(2.1, 2.9) > 0.05, "tones");
        assert!(peak(1.1, 1.9) < 0.001, "the gap is silent");
        assert!(peak(2.95, f64::MAX) > 0.05 && peak(3.05, f64::MAX) == 0.0, "the second tone ends at 3 s");
        // The second file's audio starts about 0.4 s before the first one's ends. Appending it would
        // give 2 s; placing it by time drops the overlapping start and leaves no hole.
        let joined = dir.join("overlap.ts");
        std::fs::write(&joined, [std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()].concat()).unwrap();
        let peak = pcm(&joined);
        let length = (0..).map(|i| i as f64 * 0.05).find(|&t| peak(t, f64::MAX) == 0.0).unwrap();
        assert!((1.55..1.75).contains(&length), "length {length} s");
        assert!((0..30).all(|i| peak(i as f64 * 0.05, i as f64 * 0.05 + 0.05) > 0.05), "no hole");
        std::fs::remove_dir_all(&dir).ok();
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
