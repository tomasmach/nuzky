mod qa_support;
use nuzky_engine::{
    Renderer, Wait,
    edit::{EditCmd, TimeRange},
};
use qa_support::*;
use std::time::{Duration, Instant};

/// A numeric field of /proc/self/status, such as Threads or VmRSS (kB).
fn status(field: &str) -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix(field)?.strip_prefix(':'))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// Plays a silence-cut timeline in real time like the preview does and reports decoder load.
#[test]
#[ignore = "plays 20 s in real time; run with --ignored --nocapture to measure"]
fn playback_through_many_cut_pieces_of_one_file() {
    if !available() {
        return;
    }
    let d = dir("many-pieces");
    // NUZKY_QA_SIZE=3840x2160 measures 4K sources.
    let size = std::env::var("NUZKY_QA_SIZE").unwrap_or_else(|_| "1920x1080".into());
    let path = d.join(format!("source-{size}.mp4"));
    if !path.exists() {
        ff(
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size={size}:rate=30:duration=60"),
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-g",
                "60",
                "-bf",
                "2",
                "-pix_fmt",
                "yuv420p",
            ],
            &path,
        );
    }
    let mut p = project(&path, 60_000_000);
    // Keep 0.8 s, cut 0.4 s: 50 pieces of one file, each starting 0.4 s after the previous ended.
    let ranges =
        (0..49).map(|k| TimeRange { start_us: 800_000 + k * 1_200_000, end_us: 1_200_000 + k * 1_200_000 }).collect();
    p.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
    assert_eq!(p.tracks[0].clips.len(), 50);

    let mut renderer = Renderer::new().unwrap();
    let (w, h) = (960, 540);
    renderer.render(&p, 0, w, h, Wait::Exact, false).unwrap();
    let frame_us = 1_000_000 / 30;
    let (mut max_decoders, mut max_threads, mut max_rss, mut frames) = (0, 0, 0, 0);
    let mut times = Vec::new();
    let late_before = renderer.late_layers;
    let start = Instant::now();
    let length = Duration::from_secs(20);
    while start.elapsed() < length {
        let t = start.elapsed().as_micros() as i64 / frame_us * frame_us;
        let begun = Instant::now();
        renderer.render(&p, t, w, h, Wait::Ready, true).unwrap();
        times.push(begun.elapsed());
        frames += 1;
        max_decoders = max_decoders.max(renderer.decoders());
        max_threads = max_threads.max(status("Threads"));
        max_rss = max_rss.max(status("VmRSS"));
        let next = Duration::from_micros((t + frame_us) as u64);
        std::thread::sleep(next.saturating_sub(start.elapsed()));
    }
    times.sort();
    let late = renderer.late_layers - late_before;
    eprintln!(
        "QA pieces {size}: {frames} frames, {late} late layers ({:.1} %), decoders max {max_decoders}, \
         threads max {max_threads}, RSS max {} MB, render p50 {:?} p95 {:?} max {:?}",
        late as f64 * 100.0 / frames as f64,
        max_rss / 1024,
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times.last().unwrap()
    );
}

#[test]
fn cut_pieces_of_one_file_share_a_decoder_and_show_the_right_frames() {
    if !available() {
        return;
    }
    let d = dir("pieces-share-decoder");
    let path = d.join("counter.mp4");
    ff(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=96x64:rate=25:duration=6",
            "-vf",
            "drawtext=text='%{n}':x=4:y=4:fontsize=22:fontcolor=white:box=1:boxcolor=black",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
        ],
        &path,
    );
    let timestamps = pts(&path);
    let truth = raw(&path, &[]);
    let mut p = project(&path, 6_000_000);
    let ranges = (0..9).map(|k| TimeRange { start_us: 400_000 + k * 600_000, end_us: 600_000 + k * 600_000 }).collect();
    p.apply(EditCmd::RippleDeleteRanges { ranges, keep_track_ids: None }).unwrap();
    assert_eq!(p.tracks[0].clips.len(), 10);
    let mut renderer = Renderer::new().unwrap();
    for playing in [false, true] {
        for t in (0..p.duration_us()).step_by(80_000) {
            let wait = if playing { Wait::Ready } else { Wait::Exact };
            let frame = renderer.render(&p, t, 96, 64, wait, playing).unwrap();
            // Playback also prefetches the pieces starting within the next second: up to three here.
            assert!(renderer.decoders() <= if playing { 4 } else { 1 }, "{} decoders at {t}", renderer.decoders());
            if playing {
                continue;
            }
            let clip = p.tracks[0].clips.iter().find(|c| c.contains(t)).unwrap();
            let source = nuzky_engine::effects::source_time(clip, t);
            let index = timestamps.iter().rposition(|&ts| ts <= source).unwrap();
            let error = mae(&frame, &truth[index * 96 * 64 * 4..(index + 1) * 96 * 64 * 4]);
            assert!(error < 3.0, "t={t} source={source}: MAE {error}");
        }
    }
}
