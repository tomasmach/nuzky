//! The preview thread: owns the GPU renderer, follows the audio clock while playing
//! and renders the exact frame when paused.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use capopen_engine::{Project, Renderer, Wait};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio_out::AudioOut;
use crate::preview_server::{FLAG_PLAYING, PreviewServer};

/// Longest preview side in pixels; keeps the socket well under its throughput limit.
const MAX_PREVIEW_SIDE: f32 = 1280.0;

pub enum Msg {
    Project(Arc<Project>),
    Seek(i64),
    Play,
    Pause,
    /// Available preview box in device pixels.
    Resize(u32, u32),
}

#[derive(Clone, Copy, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    pub playing: bool,
    pub t_us: i64,
}

pub struct Engine {
    tx: Sender<Msg>,
    pub transport: Arc<Mutex<Transport>>,
}

impl Engine {
    pub fn start(app: AppHandle, server: Arc<PreviewServer>, project: Arc<Project>, cache_dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let transport = Arc::new(Mutex::new(Transport::default()));
        let t = transport.clone();
        std::thread::Builder::new()
            .name("preview-engine".into())
            .spawn(move || run(rx, app, server, project, cache_dir, t))
            .expect("spawn preview engine");
        Self { tx, transport }
    }

    pub fn send(&self, msg: Msg) {
        self.tx.send(msg).ok();
    }
}

fn preview_size(project: &Project, box_w: u32, box_h: u32) -> (u32, u32) {
    let (cw, ch) = (project.canvas.width as f32, project.canvas.height as f32);
    let scale = (box_w as f32 / cw).min(box_h as f32 / ch).min(MAX_PREVIEW_SIDE / cw.max(ch));
    let even = |v: f32| ((v.round() as u32).max(32)) & !1;
    (even(cw * scale), even(ch * scale))
}

struct State {
    project: Arc<Project>,
    shared_project: Arc<Mutex<Arc<Project>>>,
    t_us: i64,
    box_size: (u32, u32),
    audio: Option<AudioOut>,
    dirty: bool,
}

fn run(
    rx: Receiver<Msg>,
    app: AppHandle,
    server: Arc<PreviewServer>,
    project: Arc<Project>,
    cache_dir: PathBuf,
    transport: Arc<Mutex<Transport>>,
) {
    let mut renderer = match Renderer::new() {
        Ok(r) => r,
        Err(e) => {
            log::error!("Preview renderer failed: {e:#}");
            app.emit("engine-error", format!("{e:#}")).ok();
            return;
        }
    };
    log::info!("Preview renderer on {}", renderer.adapter_name());
    let mut st = State {
        shared_project: Arc::new(Mutex::new(project.clone())),
        project,
        t_us: 0,
        box_size: (540, 960),
        audio: None,
        dirty: true,
    };

    let emit = |st: &State, app: &AppHandle| {
        let tr = Transport { playing: st.audio.is_some(), t_us: st.t_us };
        *transport.lock().unwrap() = tr;
        app.emit("transport", tr).ok();
    };

    loop {
        // While playing, wake up for the next frame; otherwise sleep until a message.
        let msg = if st.audio.is_some() {
            let frame = st.project.frame_duration_us();
            let next = ((st.t_us as f64 / frame).floor() + 1.0) * frame;
            let now = st.audio.as_ref().unwrap().now_us();
            let wait = Duration::from_micros((next - now as f64).clamp(0.0, 50_000.0) as u64);
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };

        let mut pending = msg.into_iter().collect::<Vec<_>>();
        pending.extend(rx.try_iter());
        let mut state_changed = false;
        for m in pending {
            match m {
                Msg::Project(p) => {
                    *st.shared_project.lock().unwrap() = p.clone();
                    st.project = p;
                    st.dirty = true;
                }
                Msg::Seek(t) => {
                    st.t_us = t.clamp(0, st.project.duration_us().max(0));
                    if st.audio.is_some() {
                        st.audio = Some(AudioOut::start(st.shared_project.clone(), st.t_us, cache_dir.clone()));
                    }
                    st.dirty = true;
                    state_changed = true;
                }
                Msg::Play if st.audio.is_none() => {
                    let end = st.project.duration_us();
                    if end <= 0 {
                        continue;
                    }
                    if st.t_us >= end - st.project.frame_duration_us() as i64 {
                        st.t_us = 0;
                    }
                    st.audio = Some(AudioOut::start(st.shared_project.clone(), st.t_us, cache_dir.clone()));
                    state_changed = true;
                }
                Msg::Play => {}
                Msg::Pause => {
                    if let Some(a) = st.audio.take() {
                        st.t_us = a.now_us().min(st.project.duration_us());
                        st.dirty = true;
                        state_changed = true;
                    }
                }
                Msg::Resize(w, h) => {
                    st.box_size = (w.max(32), h.max(32));
                    st.dirty = true;
                }
            }
        }

        let playing = st.audio.is_some();
        if playing {
            let end = st.project.duration_us();
            let now = st.audio.as_ref().unwrap().now_us();
            if now >= end {
                st.audio = None;
                st.t_us = end;
                st.dirty = true;
                state_changed = true;
            } else {
                st.t_us = now;
            }
        }
        if state_changed {
            emit(&st, &app);
        }

        let playing = st.audio.is_some();
        if playing || st.dirty {
            let (w, h) = preview_size(&st.project, st.box_size.0, st.box_size.1);
            let wait = if playing { Wait::Ready } else { Wait::Exact };
            let started = Instant::now();
            match renderer.render(&st.project, last_frame_at_end(&st.project, st.t_us), w, h, wait, playing) {
                Ok(rgba) => {
                    server.publish(w, h, if playing { FLAG_PLAYING } else { 0 }, st.t_us, &rgba);
                    let took = started.elapsed();
                    if took > Duration::from_millis(40) {
                        log::debug!("Slow preview frame: {took:?}");
                    }
                }
                Err(e) => log::error!("Preview render failed: {e:#}"),
            }
            st.dirty = false;
        }
    }
}

/// The playhead may rest on the very end, where no clip plays; the preview then keeps the last frame.
fn last_frame_at_end(project: &Project, t_us: i64) -> i64 {
    let end = project.duration_us();
    if end > 0 && t_us >= end { end - 1 } else { t_us }
}
