mod qa_support;
use capopen_engine::media::{VideoDecoder, extract_pcm, probe};
use qa_support::*;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Runs every way the engine opens media on `path` and returns the error texts. A network open
/// can wait forever for an answer, so each attempt gets a deadline.
fn open_errors(path: &Path, out: &Path) -> Vec<String> {
    let (tx, rx) = mpsc::channel();
    let (path, out) = (path.to_path_buf(), out.to_path_buf());
    std::thread::spawn(move || {
        let results = [
            probe(&path, "remote".into()).map(drop),
            VideoDecoder::open(&path).map(drop),
            extract_pcm(&path, &out, |_| Ok(())).map(drop),
        ];
        tx.send(results.map(|r| r.map_err(|e| format!("{e:#}")))).ok();
    });
    let results = rx.recv_timeout(Duration::from_secs(10)).expect("opening media hung on the network");
    results.into_iter().map(|r| r.expect_err("remote or indirect media opened")).collect()
}

fn assert_untouched(listener: &TcpListener) {
    match listener.accept() {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        other => panic!("FFmpeg connected to the local listener: {other:?}"),
    }
}

#[test]
fn media_opens_never_reach_the_network_or_follow_playlists() {
    if !available() {
        return;
    }
    let d = dir("no-network");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let out = d.join("out.f32");

    let url = PathBuf::from(format!("http://127.0.0.1:{port}/x.mp4"));
    eprintln!("{:?}", open_errors(&url, &out));
    assert_untouched(&listener);

    // A local playlist naming a remote segment, and indirections to local media.
    let local = d.join("local.ts");
    ff(&["-f", "lavfi", "-i", "testsrc2=s=64x64:r=25:d=0.4", "-c:v", "libx264", "-threads", "2"], &local);
    let remote_playlist = d.join("remote.m3u8");
    std::fs::write(
        &remote_playlist,
        format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhttp://127.0.0.1:{port}/seg.ts\n#EXT-X-ENDLIST\n"),
    )
    .unwrap();
    let local_playlist = d.join("local.m3u8");
    std::fs::write(
        &local_playlist,
        format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n{}\n#EXT-X-ENDLIST\n", local.display()),
    )
    .unwrap();
    let concat = d.join("list.ffconcat");
    std::fs::write(&concat, format!("ffconcat version 1.0\nfile '{}'\n", local.display())).unwrap();
    for path in [&remote_playlist, &local_playlist, &concat] {
        eprintln!("{}: {:?}", path.display(), open_errors(path, &out));
    }
    assert_untouched(&listener);
    assert!(!out.exists());

    // Plain local media still opens.
    assert!(probe(&local, "local".into()).is_ok());
    assert!(VideoDecoder::open(&local).is_ok());
}
