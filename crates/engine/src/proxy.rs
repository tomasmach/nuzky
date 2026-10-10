//! Preview proxies: a light H.264 copy of video that is slow to decode, such as HEVC from phones, made once
//! in the background. The preview and thumbnails decode it instead of the original once it is there; export
//! never reads it. A proxy keeps the original's frame times, colours and stored orientation, so the two
//! swap without moving a cut, and differs from it only by compression. It is a pure cache, named after the
//! file's path, size and time: a missing one only makes the preview slower.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use ff::software::scaling;
use ff::util::{color, format::Pixel, frame};
use ffmpeg_next as ff;

use crate::audio::lock_cache;
use crate::media::{
    VideoDecoder, file_key, is_full_range, is_image_format, matrix, open_input, set_sws_colorspace, sws_colorspace,
    video_stream,
};

/// Raised whenever proxies are made differently, so ones made before are made again.
pub const PROXY_VERSION: &str = "v1";
/// Proxies are at most 1080p: the preview is at most 1280 px on its longer side.
const SHORT_SIDE: u32 = 1080;
/// A keyframe every 15 frames, so a jump decodes at most 14 frames before the one it shows.
const GOP: u32 = 15;
/// Frame times in microseconds, the unit `VideoDecoder` reports them in.
const TIME_BASE: (i32, i32) = (1, 1_000_000);

pub fn proxy_path(cache_dir: &Path, source: &Path) -> PathBuf {
    cache_dir.join("proxy").join(format!("{}.{PROXY_VERSION}.mp4", file_key(&source.to_string_lossy())))
}

/// The proxy of `source`, when it has been made.
pub fn ready(cache_dir: &Path, source: &Path) -> Option<PathBuf> {
    Some(proxy_path(cache_dir, source)).filter(|path| path.exists())
}

/// The decoder the preview reads `source` with: its proxy once made, else the file itself.
pub fn open_decoder(cache_dir: &Path, source: &Path) -> Result<VideoDecoder> {
    match ready(cache_dir, source) {
        Some(proxy) => VideoDecoder::open_proxy(&proxy),
        None => VideoDecoder::open(source),
    }
}

/// Whether the video at `path` should preview from a proxy: HEVC, AV1 and VP9, which phones record and which
/// decode slowly, and anything larger than a proxy. Not stills, nor video with transparency, which a proxy
/// would show opaque.
pub fn wanted(path: &Path) -> Result<bool> {
    let input = open_input(path)?;
    let Some(stream) = video_stream(&input).filter(|_| !is_image_format(&input)) else { return Ok(false) };
    if ff::encoder::find_by_name("libx264").is_none() {
        return Ok(false);
    }
    let heavy = matches!(stream.parameters().id(), ff::codec::Id::HEVC | ff::codec::Id::AV1 | ff::codec::Id::VP9);
    let decoder = ff::codec::context::Context::from_parameters(stream.parameters())?.decoder().video()?;
    let alpha = decoder
        .format()
        .descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).flags } & ff::ffi::AV_PIX_FMT_FLAG_ALPHA as u64 != 0);
    Ok(!alpha && (heavy || decoder.width().min(decoder.height()) > SHORT_SIDE))
}

/// Makes the proxy of `source` unless it exists. Concurrent callers wait for one of them. An error from
/// `progress` stops it and leaves nothing behind; the next call starts again.
pub fn ensure_proxy(cache_dir: &Path, source: &Path, mut progress: impl FnMut(f32) -> Result<()>) -> Result<PathBuf> {
    let path = proxy_path(cache_dir, source);
    let _lock = lock_cache(&path, &mut progress)?;
    if !path.exists() {
        let tmp = path.with_extension(format!("mp4.{}.part", crate::edit::new_id()));
        let result = transcode(source, &tmp, &mut progress).and_then(|()| Ok(std::fs::rename(&tmp, &path)?));
        if result.is_err() {
            std::fs::remove_file(&tmp).ok();
        }
        result?;
        remove_others(&path);
    }
    Ok(path)
}

/// Removes the proxies of earlier versions of this file, or of this format, and unfinished ones: nothing
/// reads them any more. Names are `<size>-<time>-<path hash>.<version>.mp4`.
fn remove_others(path: &Path) {
    let source = |name: &str| name.split('.').next().and_then(|key| key.rsplit('-').next()).map(str::to_owned);
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().map(|n| n.to_string_lossy().into_owned())) else {
        return;
    };
    let lock = format!("{name}.lock");
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if other != name && other != lock && source(&other) == source(&name) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

fn transcode(source: &Path, out: &Path, progress: &mut impl FnMut(f32) -> Result<()>) -> Result<()> {
    let codec = ff::encoder::find_by_name("libx264").context("Preview proxies need the libx264 encoder")?;
    let mut decoder = VideoDecoder::open(source)?;
    let duration = decoder.duration_us().max(1);
    let mut octx = ff::format::output_as(out, "mp4").with_context(|| format!("Cannot create {}", out.display()))?;
    let mut encoder = None;
    let mut last = i64::MIN;
    while let Some((t, f)) = decoder.next_frame()? {
        // A frame at the time of the one before is never shown, and H.264 needs rising times.
        if t <= last {
            continue;
        }
        last = t;
        if encoder.is_none() {
            encoder = Some(open_encoder(codec, &f, decoder.frame_duration_us, &mut octx)?);
        }
        let (enc, scaler, yuv) = encoder.as_mut().unwrap();
        scaler.run(&f, yuv)?;
        yuv.set_pts(Some(t));
        enc.send_frame(yuv)?;
        write_packets(enc, &mut octx, decoder.frame_duration_us)?;
        progress(t as f32 / duration as f32)?;
    }
    let Some((mut enc, ..)) = encoder else { bail!("{} has no video frames", source.display()) };
    enc.send_eof()?;
    write_packets(&mut enc, &mut octx, decoder.frame_duration_us)?;
    octx.write_trailer()?;
    Ok(())
}

/// The encoder, with the colours of the original's first frame, and the scaler to the proxy's size.
fn open_encoder(
    codec: ff::Codec,
    f: &frame::Video,
    frame_us: i64,
    octx: &mut ff::format::context::Output,
) -> Result<(ff::encoder::video::Encoder, scaling::Context, frame::Video)> {
    let scale = (SHORT_SIDE as f64 / f.width().min(f.height()) as f64).min(1.0);
    let even = |v: u32| ((v as f64 * scale / 2.0).round() as u32).max(1) * 2;
    let (w, h) = (even(f.width()), even(f.height()));
    // Named, so the smaller proxy is not shown with the matrix guessed for its own height.
    let (space, full) = (matrix(f), is_full_range(f));
    let mut enc = ff::codec::context::Context::new_with_codec(codec).encoder().video()?;
    enc.set_width(w);
    enc.set_height(h);
    enc.set_format(Pixel::YUV420P);
    enc.set_time_base(TIME_BASE);
    enc.set_frame_rate(Some((1_000_000, frame_us.clamp(1, i32::MAX as i64) as i32)));
    enc.set_gop(GOP);
    enc.set_max_b_frames(0);
    enc.set_colorspace(space);
    enc.set_color_range(if full { color::Range::JPEG } else { color::Range::MPEG });
    unsafe {
        let ctx = enc.as_mut_ptr();
        (*ctx).color_primaries = f.color_primaries().into();
        (*ctx).color_trc = f.color_transfer_characteristic().into();
    }
    if octx.format().flags().contains(ff::format::Flags::GLOBAL_HEADER) {
        enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut options = ff::Dictionary::new();
    options.set("preset", "veryfast");
    options.set("tune", "fastdecode");
    options.set("crf", "20");
    // Decoding the original is the slow part; the encoder leaves the other cores to the preview.
    options.set("threads", "2");
    let enc = enc.open_with(options).context("Opening the H.264 encoder for the preview proxy failed")?;
    let mut stream = octx.add_stream(codec)?;
    stream.set_parameters(&enc);
    stream.set_time_base(TIME_BASE);
    // The edit list that starts the picture after the sound is in the movie's units, by default milliseconds.
    let mut header = ff::Dictionary::new();
    header.set("movie_timescale", &TIME_BASE.1.to_string());
    octx.write_header_with(header)?;
    let mut scaler =
        scaling::Context::get(f.format(), f.width(), f.height(), Pixel::YUV420P, w, h, scaling::Flags::BICUBIC)?;
    // Only the size and bit depth change: the same matrix and range on both sides.
    set_sws_colorspace(&mut scaler, sws_colorspace(space), full, sws_colorspace(space), full);
    Ok((enc, scaler, frame::Video::new(Pixel::YUV420P, w, h)))
}

fn write_packets(
    enc: &mut ff::encoder::video::Encoder,
    octx: &mut ff::format::context::Output,
    frame_us: i64,
) -> Result<()> {
    let time_base = octx.stream(0).unwrap().time_base();
    let mut packet = ff::Packet::empty();
    while enc.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        // Every frame but the last lasts until the next one; the muxer takes the last one's from here.
        packet.set_duration(frame_us);
        packet.rescale_ts(TIME_BASE, time_base);
        packet.write(octx)?;
    }
    Ok(())
}
