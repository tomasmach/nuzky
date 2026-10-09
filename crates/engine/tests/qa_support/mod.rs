#![allow(dead_code)]
use nuzky_engine::{media::probe, model::*};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub fn dir(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test/qa").join(name);
    std::fs::create_dir_all(&p).unwrap();
    p
}
pub fn available() -> bool {
    if Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        true
    } else {
        eprintln!("SKIP: ffmpeg CLI is unavailable");
        false
    }
}
pub fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(out.status.success(), "{cmd:?}\n{}", String::from_utf8_lossy(&out.stderr));
    out
}
pub fn ff(args: &[&str], out: &Path) {
    run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-threads", "2", "-filter_threads", "1"])
        .args(args)
        .arg(out));
}
pub fn raw(path: &Path, extra: &[&str]) -> Vec<u8> {
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "2", "-filter_threads", "1", "-i"])
        .arg(path)
        .args(extra)
        .args(["-sws_flags", "bilinear", "-f", "rawvideo", "-pix_fmt", "rgba", "-fps_mode", "passthrough", "-"]))
    .stdout
}
pub fn info(path: &Path) -> serde_json::Value {
    let o =
        run(Command::new("ffprobe").args(["-v", "error", "-show_streams", "-show_format", "-of", "json"]).arg(path));
    serde_json::from_slice(&o.stdout).unwrap()
}
pub fn pts(path: &Path) -> Vec<i64> {
    let o = run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time",
            "-of",
            "json",
        ])
        .arg(path));
    let j: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let origin = info(path)["format"]["start_time"].as_str().unwrap_or("0").parse::<f64>().unwrap();
    j["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            ((f["best_effort_timestamp_time"].as_str().unwrap().parse::<f64>().unwrap() - origin) * 1e6).round() as i64
        })
        .collect()
}
pub fn clip(id: &str, asset: &str, start: i64, duration: i64) -> Clip {
    Clip::new(
        id.into(),
        start,
        duration,
        ClipContent::Media {
            asset_id: asset.into(),
            source_in_us: 0,
            volume: 1.0,
            transform: Transform::default(),
            speed: 1.0,
            adjust: Adjust::default(),
            fade_in_us: 0,
            fade_out_us: 0,
            clean_voice: false,
            shape: None,
        },
    )
}
pub fn project(path: &Path, duration: i64) -> Project {
    let mut p = Project::new("QA");
    let a = probe(path, "qa-source".into()).unwrap();
    p.canvas.width = a.width.max(2);
    p.canvas.height = a.height.max(2);
    p.tracks[0].clips.push(clip("qa-clip", &a.id, 0, duration));
    p.assets.push(a);
    p
}
pub fn mae(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).abs()).sum::<f64>() / a.len() as f64
}
pub fn pcm(path: &Path) -> Vec<f32> {
    std::fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect()
}
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
