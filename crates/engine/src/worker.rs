//! Background decoding: one thread per clip keeps a short queue of converted frames
//! around the requested time, so the render thread never waits on a seek it can skip.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::media::{RgbaFrame, VideoDecoder};

const LOOKAHEAD_PLAYING: usize = 4;
/// Jumping further ahead than this seeks instead of decoding through.
const FORWARD_SEEK_US: i64 = 1_000_000;
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Default)]
struct State {
    want_us: i64,
    size: (u32, u32),
    playing: bool,
    /// Ascending by time. frames[i] is shown for [frames[i].t, frames[i+1].t).
    frames: VecDeque<RgbaFrame>,
    /// Time the queue was seeked to; frames[0] also covers [base_us, frames[0].t).
    base_us: i64,
    /// The decoder has no frames after the last queued one.
    eof: bool,
    /// Bumped when the queue is invalidated, so in-flight work is discarded.
    epoch: u64,
    error: Option<String>,
    shutdown: bool,
    source_size: Option<(u32, u32)>,
    rotation: u32,
}

impl State {
    fn covering(&self, t: i64) -> Option<(usize, bool)> {
        let first = self.frames.front()?;
        if t < self.base_us {
            return None;
        }
        let idx = self.frames.iter().rposition(|f| f.t_us <= t).unwrap_or(0);
        if t < first.t_us && idx == 0 {
            // Between the seek point and the first real frame.
            return Some((0, true));
        }
        let exact = idx + 1 < self.frames.len() || self.eof;
        Some((idx, exact))
    }

    fn needs_seek(&self) -> bool {
        match (self.frames.front(), self.frames.back()) {
            (Some(_), Some(last)) => {
                self.want_us < self.base_us || (!self.eof && self.want_us > last.t_us + FORWARD_SEEK_US)
            }
            _ => true,
        }
    }

    fn needs_more(&self) -> bool {
        if self.eof {
            return false;
        }
        let ahead = self.frames.iter().filter(|f| f.t_us > self.want_us).count();
        ahead == 0 || (self.playing && ahead < LOOKAHEAD_PLAYING)
    }

    fn has_work(&self) -> bool {
        self.size.0 > 0 && self.error.is_none() && (self.needs_seek() || self.needs_more())
    }
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

pub struct VideoWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    pub last_used: Instant,
    pub exact: bool,
}

impl VideoWorker {
    pub fn spawn(path: PathBuf) -> Self {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), cv: Condvar::new() });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || run(s, path))
            .expect("spawn decoder thread");
        Self { shared, thread: Some(thread), last_used: Instant::now(), exact: false }
    }

    /// Source size (before rotation) once the file is open.
    pub fn source_info(&self) -> Option<((u32, u32), u32)> {
        let st = self.shared.state.lock().unwrap();
        st.source_size.map(|s| (s, st.rotation))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.state.lock().unwrap().error.clone()
    }

    /// Returns the frame to show at `t_us`. With `blocking`, waits for the exact frame;
    /// otherwise returns the nearest ready frame or `None` and lets decoding catch up.
    pub fn get(&mut self, t_us: i64, size: (u32, u32), playing: bool, blocking: bool) -> Option<RgbaFrame> {
        self.last_used = Instant::now();
        self.exact = false;
        let deadline = Instant::now() + BLOCKING_TIMEOUT;
        let mut st = self.shared.state.lock().unwrap();
        if st.size != size {
            st.size = size;
            st.frames.clear();
            st.epoch += 1;
        }
        st.want_us = t_us;
        st.playing = playing;
        loop {
            if let Some((idx, exact)) = st.covering(t_us) {
                // Drop frames that can no longer be shown.
                for _ in 0..idx {
                    st.frames.pop_front();
                }
                if idx > 0 {
                    st.base_us = st.frames[0].t_us;
                }
                let frame = st.frames[0].clone();
                if exact || !blocking {
                    self.exact = exact;
                    self.shared.cv.notify_all();
                    return Some(frame);
                }
            }
            self.shared.cv.notify_all();
            if !blocking || st.error.is_some() {
                return st.frames.front().cloned();
            }
            let now = Instant::now();
            if now >= deadline {
                log::warn!("Timed out waiting for frame at {t_us} us");
                return st.frames.front().cloned();
            }
            st = self.shared.cv.wait_timeout(st, deadline - now).unwrap().0;
        }
    }
}

impl Drop for VideoWorker {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

fn run(shared: Arc<Shared>, path: PathBuf) {
    let mut decoder = match VideoDecoder::open(&path) {
        Ok(d) => d,
        Err(e) => {
            let mut st = shared.state.lock().unwrap();
            st.error = Some(format!("{e:#}"));
            shared.cv.notify_all();
            return;
        }
    };
    {
        let mut st = shared.state.lock().unwrap();
        st.source_size = Some(decoder.source_size());
        st.rotation = decoder.rotation;
        shared.cv.notify_all();
    }

    loop {
        let (want, size, seek, epoch) = {
            let mut st = shared.state.lock().unwrap();
            while !st.shutdown && !st.has_work() {
                st = shared.cv.wait(st).unwrap();
            }
            if st.shutdown {
                return;
            }
            (st.want_us, st.size, st.needs_seek(), st.epoch)
        };

        let result = if seek { seek_to(&mut decoder, want, size) } else { decode_next(&mut decoder, size) };
        let mut st = shared.state.lock().unwrap();
        if st.epoch != epoch {
            continue;
        }
        match result {
            Ok(Step::Seeked { frames, eof }) => {
                st.frames = frames.into();
                st.base_us = want;
                st.eof = eof;
            }
            Ok(Step::Next(Some(frame))) => st.frames.push_back(frame),
            Ok(Step::Next(None)) => st.eof = true,
            Err(e) => {
                log::error!("Decoding {} failed: {e:#}", path.display());
                st.error = Some(format!("{e:#}"));
            }
        }
        shared.cv.notify_all();
    }
}

enum Step {
    Seeked { frames: Vec<RgbaFrame>, eof: bool },
    Next(Option<RgbaFrame>),
}

/// Seeks and decodes until the frame covering `want` and the one after it are known.
/// Only those two get converted, so seeking through a long GOP stays cheap.
fn seek_to(decoder: &mut VideoDecoder, want: i64, size: (u32, u32)) -> anyhow::Result<Step> {
    decoder.seek(want)?;
    let mut candidate = None;
    loop {
        match decoder.next_frame()? {
            Some((t, f)) if t <= want || candidate.is_none() && decoder.is_image() => candidate = Some((t, f)),
            Some((t, f)) => {
                let mut frames = Vec::with_capacity(2);
                if let Some((ct, cf)) = candidate {
                    frames.push(decoder.convert(&cf, ct, size.0, size.1)?);
                }
                frames.push(decoder.convert(&f, t, size.0, size.1)?);
                return Ok(Step::Seeked { frames, eof: false });
            }
            None => {
                let frames = match candidate {
                    Some((ct, cf)) => vec![decoder.convert(&cf, ct, size.0, size.1)?],
                    None => Vec::new(),
                };
                return Ok(Step::Seeked { frames, eof: true });
            }
        }
    }
}

fn decode_next(decoder: &mut VideoDecoder, size: (u32, u32)) -> anyhow::Result<Step> {
    Ok(Step::Next(match decoder.next_frame()? {
        Some((t, f)) => Some(decoder.convert(&f, t, size.0, size.1)?),
        None => None,
    }))
}
