//! Pushes preview frames to the webview over a loopback WebSocket.
//!
//! Tauri IPC routes large payloads through the GTK main thread on Linux, so frames take
//! this side channel instead. The socket is send-only, bound to 127.0.0.1 on a random
//! port, and only accepts the app's own origin with a per-launch secret path.

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use tungstenite::handshake::server::{Callback, ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Bytes, Message};

const ALLOWED_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];
/// The Vite dev server serves the UI only in debug builds.
const DEV_ORIGIN: &str = "http://localhost:1420";

fn allowed_origin(origin: &str) -> bool {
    ALLOWED_ORIGINS.contains(&origin) || (cfg!(debug_assertions) && origin == DEV_ORIGIN)
}

/// Header layout (little endian): magic "CPF1", width u32, height u32, flags u32, time i64.
pub const HEADER_LEN: usize = 24;
pub const FLAG_PLAYING: u32 = 1;

#[derive(Default)]
struct Slot {
    frame: Mutex<(u64, Option<Bytes>)>,
    cv: Condvar,
}

pub struct PreviewServer {
    pub url: String,
    slot: Arc<Slot>,
}

impl PreviewServer {
    pub fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let slot = Arc::new(Slot::default());
        let (s, path) = (slot.clone(), format!("/{token}"));
        thread::Builder::new().name("preview-accept".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, path) = (s.clone(), path.clone());
                thread::Builder::new().name("preview-ws".into()).spawn(move || serve(stream, &path, s)).ok();
            }
        })?;
        Ok(Self { url: format!("ws://127.0.0.1:{port}/{token}"), slot })
    }

    /// Replaces the pending frame. Slow clients skip straight to the newest one.
    pub fn publish(&self, width: u32, height: u32, flags: u32, t_us: i64, rgba: &[u8]) {
        let mut buf = Vec::with_capacity(HEADER_LEN + rgba.len());
        buf.extend_from_slice(b"CPF1");
        buf.extend_from_slice(&width.to_le_bytes());
        buf.extend_from_slice(&height.to_le_bytes());
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&t_us.to_le_bytes());
        buf.extend_from_slice(rgba);
        let mut f = self.slot.frame.lock().unwrap();
        f.0 += 1;
        f.1 = Some(Bytes::from(buf));
        self.slot.cv.notify_all();
    }
}

struct PreviewHandshake<'a>(&'a str);

impl Callback for PreviewHandshake<'_> {
    fn on_request(self, req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|o| o.to_str().ok()).unwrap_or("");
        if req.uri().path() != self.0 || !allowed_origin(origin) {
            log::warn!("Rejected preview connection from origin {origin:?}");
            let mut e = ErrorResponse::new(None);
            *e.status_mut() = StatusCode::FORBIDDEN;
            return Err(e);
        }
        Ok(resp)
    }
}

fn serve(stream: TcpStream, path: &str, slot: Arc<Slot>) {
    stream.set_nodelay(true).ok();
    let Ok(mut ws) = tungstenite::accept_hdr(stream, PreviewHandshake(path)) else { return };
    let mut sent = 0;
    loop {
        let frame = {
            let mut g = slot.frame.lock().unwrap();
            while g.0 == sent {
                let (next, timeout) = slot.cv.wait_timeout(g, Duration::from_secs(2)).unwrap();
                g = next;
                if timeout.timed_out() && g.0 == sent {
                    drop(g);
                    // Detects closed clients so their threads end.
                    if ws.send(Message::Ping(Bytes::new())).is_err() {
                        return;
                    }
                    g = slot.frame.lock().unwrap();
                }
            }
            sent = g.0;
            g.1.clone()
        };
        if let Some(f) = frame
            && ws.send(Message::Binary(f)).is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_server_origin_is_allowed_only_in_debug_builds() {
        assert!(allowed_origin("tauri://localhost"));
        assert_eq!(allowed_origin(DEV_ORIGIN), cfg!(debug_assertions));
        assert!(!allowed_origin("http://localhost:1421") && !allowed_origin(""));
    }
}
