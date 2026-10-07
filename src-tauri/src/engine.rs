//! The preview thread: owns the GPU renderer, follows the audio clock while playing
//! and renders the exact frame when paused.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
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

/// Where the preview thread reports playback and errors: the webview in the app.
trait Events: Send + 'static {
    fn transport(&self, transport: Transport);
    /// `None` once the preview renders again after an error.
    fn error(&self, error: Option<&str>);
}

impl Events for AppHandle {
    fn transport(&self, transport: Transport) {
        self.emit("transport", transport).ok();
    }

    fn error(&self, error: Option<&str>) {
        self.emit("engine-error", error).ok();
    }
}

/// Draws preview frames: the GPU renderer in the app.
trait Frames {
    fn frame(&mut self, project: &Project, t_us: i64, w: u32, h: u32, wait: Wait, playing: bool) -> Result<Vec<u8>>;
}

impl Frames for Renderer {
    fn frame(&mut self, project: &Project, t_us: i64, w: u32, h: u32, wait: Wait, playing: bool) -> Result<Vec<u8>> {
        self.render(project, t_us, w, h, wait, playing)
    }
}

pub struct Engine {
    tx: Sender<Msg>,
    pub transport: Arc<Mutex<Transport>>,
    /// Why the preview is unavailable; kept for a UI that subscribes after the event.
    pub error: Arc<Mutex<Option<String>>>,
}

impl Engine {
    pub fn start(app: AppHandle, server: Arc<PreviewServer>, project: Arc<Project>, cache_dir: PathBuf) -> Self {
        let renderer = || Renderer::new().inspect(|r| log::info!("Preview renderer on {}", r.adapter_name()));
        Self::start_with(app, server, project, cache_dir, renderer)
    }

    fn start_with<F: Frames>(
        events: impl Events,
        server: Arc<PreviewServer>,
        project: Arc<Project>,
        cache_dir: PathBuf,
        renderer: impl Fn() -> Result<F> + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let transport = Arc::new(Mutex::new(Transport::default()));
        let error = Arc::new(Mutex::new(None));
        let (t, e) = (transport.clone(), error.clone());
        std::thread::Builder::new()
            .name("preview-engine".into())
            .spawn(move || run(rx, events, server, project, cache_dir, t, e, renderer))
            .expect("spawn preview engine");
        Self { tx, transport, error }
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

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown")
}

#[allow(clippy::too_many_arguments)]
fn run<F: Frames>(
    rx: Receiver<Msg>,
    events: impl Events,
    server: Arc<PreviewServer>,
    project: Arc<Project>,
    cache_dir: PathBuf,
    transport: Arc<Mutex<Transport>>,
    error: Arc<Mutex<Option<String>>>,
    make_renderer: impl Fn() -> Result<F>,
) {
    let report = |message: Option<String>| {
        *error.lock().unwrap() = message.clone();
        events.error(message.as_deref());
    };
    let renderer_or_report = || {
        make_renderer().inspect_err(|e| {
            log::error!("Preview renderer failed: {e:#}");
            report(Some(format!("{e:#}")));
        })
    };
    let Ok(mut renderer) = renderer_or_report() else { return };
    let mut st = State {
        shared_project: Arc::new(Mutex::new(project.clone())),
        project,
        t_us: 0,
        box_size: (540, 960),
        audio: None,
        dirty: true,
    };

    let emit = |st: &State| {
        let tr = Transport { playing: st.audio.is_some(), t_us: st.t_us };
        *transport.lock().unwrap() = tr;
        events.transport(tr);
    };

    loop {
        // While playing, wake up for the next frame; otherwise sleep until a message.
        let msg = if let Some(audio) = &st.audio {
            let frame = st.project.frame_duration_us();
            let next = ((st.t_us as f64 / frame).floor() + 1.0) * frame;
            let now = audio.now_us();
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
        // A panic must not end the preview: one bad frame or project would otherwise need a restart.
        let step = catch_unwind(AssertUnwindSafe(|| {
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

            if let Some(audio) = &st.audio {
                let end = st.project.duration_us();
                let now = audio.now_us();
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
                emit(&st);
            }

            let playing = st.audio.is_some();
            if playing || st.dirty {
                let (w, h) = preview_size(&st.project, st.box_size.0, st.box_size.1);
                let wait = if playing { Wait::Ready } else { Wait::Exact };
                let started = Instant::now();
                match renderer.frame(&st.project, last_frame_at_end(&st.project, st.t_us), w, h, wait, playing) {
                    Ok(rgba) => {
                        server.publish(w, h, if playing { FLAG_PLAYING } else { 0 }, st.t_us, &rgba);
                        if error.lock().unwrap().is_some() {
                            report(None);
                        }
                        let took = started.elapsed();
                        if took > Duration::from_millis(40) {
                            log::debug!("Slow preview frame: {took:?}");
                        }
                    }
                    Err(e) => log::error!("Preview render failed: {e:#}"),
                }
                st.dirty = false;
            }
        }));
        if let Err(panic) = step {
            let reason = panic_message(&*panic);
            log::error!("Preview panicked: {reason}");
            report(Some(format!("Rendering failed ({reason}). Change the project or move the playhead to try again.")));
            // The renderer may have stopped half way through a frame, so the next one starts fresh.
            // Playback stops and nothing renders until the next message, so a bad frame cannot loop.
            let broken = renderer;
            catch_unwind(AssertUnwindSafe(move || drop(broken))).ok();
            let Ok(fresh) = renderer_or_report() else { return };
            renderer = fresh;
            st.audio = None;
            st.dirty = false;
            emit(&st);
        }
    }
}

/// The playhead may rest on the very end, where no clip plays; the preview then keeps the last frame.
fn last_frame_at_end(project: &Project, t_us: i64) -> i64 {
    let end = project.duration_us();
    if end > 0 && t_us >= end { end - 1 } else { t_us }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Recorder(Arc<Mutex<Vec<Option<String>>>>);

    impl Events for Recorder {
        fn transport(&self, _: Transport) {}

        fn error(&self, error: Option<&str>) {
            self.0.lock().unwrap().push(error.map(str::to_owned));
        }
    }

    /// Panics on one project, like a renderer bug that a particular project triggers.
    struct Fake;

    impl Frames for Fake {
        fn frame(&mut self, project: &Project, _: i64, w: u32, h: u32, _: Wait, _: bool) -> Result<Vec<u8>> {
            assert_ne!(project.name, "broken", "bad project");
            Ok(vec![0; (w * h * 4) as usize])
        }
    }

    #[test]
    fn a_panic_is_reported_and_the_preview_renders_the_next_project() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let made = Arc::new(AtomicUsize::new(0));
        let counter = made.clone();
        let engine = Engine::start_with(
            Recorder(events.clone()),
            Arc::new(PreviewServer::start().unwrap()),
            Arc::new(Project::new("broken")),
            std::env::temp_dir(),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Fake)
            },
        );
        let wait_for = |count: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while events.lock().unwrap().len() < count {
                assert!(Instant::now() < deadline, "events: {:?}", events.lock().unwrap());
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(100));
            events.lock().unwrap().clone()
        };
        engine.send(Msg::Resize(64, 64));
        let first = wait_for(1);
        assert_eq!(first.len(), 1, "a broken frame is tried once, not in a loop");
        assert!(first[0].as_deref().is_some_and(|e| e.contains("bad project")), "{first:?}");
        assert!(engine.error.lock().unwrap().is_some());
        // Later commands are still handled: the same project fails again, a fixed one renders.
        engine.send(Msg::Seek(0));
        assert_eq!(wait_for(2).len(), 2);
        engine.send(Msg::Project(Arc::new(Project::new("fixed"))));
        assert_eq!(wait_for(3).last(), Some(&None));
        assert!(engine.error.lock().unwrap().is_none());
        assert_eq!(made.load(Ordering::SeqCst), 3, "each panic starts a fresh renderer");
    }
}
