//! Audio output for playback. The sound card is the master clock: the playhead is
//! derived from how many mixed samples the device has consumed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use capopen_engine::Project;
use capopen_engine::audio::{Mixer, us_to_samples};
use capopen_engine::model::{CHANNELS, SAMPLE_RATE};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const CHUNK: usize = 1024;
const RING_FRAMES: usize = SAMPLE_RATE as usize / 5; // 200 ms

#[derive(Default)]
struct Clock {
    consumed: AtomicU64,
    /// Nanoseconds since `started` at the last callback.
    last_callback_ns: AtomicU64,
    latency_us: AtomicI64,
}

pub struct AudioOut {
    stream: Option<cpal::Stream>,
    clock: Arc<Clock>,
    stop: Arc<AtomicBool>,
    mixer: Option<JoinHandle<()>>,
    start_us: i64,
    started: Instant,
}

impl AudioOut {
    pub fn start(project: Arc<Mutex<Arc<Project>>>, start_us: i64, cache_dir: PathBuf) -> Self {
        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(RING_FRAMES * CHANNELS);
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();

        let stop_mix = stop.clone();
        let mixer = std::thread::Builder::new()
            .name("audio-mix".into())
            .spawn(move || {
                let mut mixer = Mixer::new(cache_dir);
                let mut pos = us_to_samples(start_us);
                let mut buf = vec![0f32; CHUNK * CHANNELS];
                while !stop_mix.load(Ordering::Relaxed) {
                    if producer.slots() < buf.len() {
                        std::thread::sleep(Duration::from_millis(4));
                        continue;
                    }
                    let snapshot = project.lock().unwrap().clone();
                    mixer.mix(&snapshot, pos, &mut buf);
                    if let Ok(chunk) = producer.write_chunk_uninit(buf.len()) {
                        chunk.fill_from_iter(buf.iter().copied());
                    }
                    pos += CHUNK as i64;
                }
            })
            .ok();

        let clock = Arc::new(Clock::default());
        let stream = open_stream(clock.clone(), started, move |out: &mut [f32]| {
            let n = out.len().min(consumer.slots());
            if let Ok(chunk) = consumer.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
            }
            out[n..].fill(0.0);
            n / CHANNELS
        });
        if stream.is_none() {
            log::warn!("No audio output; playback runs silent on the system clock");
        }
        Self { stream, clock, stop, mixer, start_us, started }
    }

    pub fn now_us(&self) -> i64 {
        if self.stream.is_none() {
            return self.start_us + self.started.elapsed().as_micros() as i64;
        }
        let consumed = self.clock.consumed.load(Ordering::Acquire) as i64;
        let at = Duration::from_nanos(self.clock.last_callback_ns.load(Ordering::Acquire));
        // Interpolate between callbacks so the clock moves smoothly.
        let since = self.started.elapsed().saturating_sub(at).min(Duration::from_millis(60)).as_micros() as i64;
        let played = consumed * 1_000_000 / SAMPLE_RATE as i64;
        let latency = self.clock.latency_us.load(Ordering::Relaxed);
        if consumed == 0 {
            return self.start_us;
        }
        self.start_us + (played + since - latency).max(0)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.stream.take();
        if let Some(m) = self.mixer.take() {
            m.join().ok();
        }
    }
}

/// Opens the default output at 48 kHz stereo f32 when possible. `fill` writes interleaved
/// stereo and returns how many timeline frames it consumed.
fn open_stream(
    clock: Arc<Clock>,
    started: Instant,
    mut fill: impl FnMut(&mut [f32]) -> usize + Send + 'static,
) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let supports_native = device.supported_output_configs().ok()?.any(|c| {
        c.channels() as usize == CHANNELS
            && c.sample_format() == cpal::SampleFormat::F32
            && c.min_sample_rate() <= SAMPLE_RATE
            && c.max_sample_rate() >= SAMPLE_RATE
    });
    let (config, channels, rate) = if supports_native {
        (cpal::StreamConfig { channels: CHANNELS as u16, sample_rate: SAMPLE_RATE, buffer_size: cpal::BufferSize::Default }, CHANNELS, SAMPLE_RATE)
    } else {
        let def = device.default_output_config().ok()?;
        if def.sample_format() != cpal::SampleFormat::F32 {
            log::warn!("Audio device does not offer f32 output");
            return None;
        }
        let c = def.config();
        (c.clone(), c.channels as usize, c.sample_rate)
    };

    // Nearest-sample rate conversion and channel mapping for unusual devices.
    let step = SAMPLE_RATE as f64 / rate as f64;
    let mut phase = 0.0f64;
    let mut scratch: Vec<f32> = Vec::new();
    let mut last = [0f32; 2];
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let frames_out = out.len() / channels;
                let consumed = if channels == CHANNELS && step == 1.0 {
                    fill(out)
                } else {
                    let need = ((phase + frames_out as f64 * step).floor() - phase.floor()) as usize;
                    scratch.resize(need * CHANNELS, 0.0);
                    let got = fill(&mut scratch);
                    let mut src = 0usize;
                    for f in 0..frames_out {
                        let next = ((phase + (f + 1) as f64 * step).floor() - phase.floor()) as usize;
                        while src < next && src < need {
                            last = [scratch[src * 2], scratch[src * 2 + 1]];
                            src += 1;
                        }
                        let frame = &mut out[f * channels..(f + 1) * channels];
                        match channels {
                            1 => frame[0] = (last[0] + last[1]) * 0.5,
                            _ => {
                                frame[0] = last[0];
                                frame[1] = last[1];
                                frame[2..].fill(0.0);
                            }
                        }
                    }
                    phase += frames_out as f64 * step;
                    got
                };
                let ts = info.timestamp();
                if let Some(lat) = ts.playback.checked_duration_since(ts.callback) {
                    clock.latency_us.store(lat.as_micros() as i64, Ordering::Relaxed);
                }
                clock.consumed.fetch_add(consumed as u64, Ordering::AcqRel);
                clock.last_callback_ns.store(started.elapsed().as_nanos() as u64, Ordering::Release);
            },
            |e| log::error!("Audio stream error: {e}"),
            None,
        )
        .map_err(|e| log::warn!("Cannot open audio output: {e}"))
        .ok()?;
    stream.play().map_err(|e| log::warn!("Cannot start audio output: {e}")).ok()?;
    Some(stream)
}
