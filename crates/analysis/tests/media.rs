use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use capopen_analysis::{
    SceneParams, SilenceParams, loudness, scene_cuts, silences, speech_segments,
};
use capopen_engine::media::probe;

fn directory(name: &str) -> Result<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .canonicalize()?;
    let root = src.ancestors().nth(3).context("Missing workspace root")?;
    let dir = root
        .join("tmp-test/analysis")
        .join(format!("tests-{}", std::process::id()))
        .join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn ffmpeg(args: &[&str], output: &Path) -> Result<()> {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(output)
        .output()
        .context("Starting ffmpeg; required for scene integration tests")?;
    ensure!(
        result.status.success(),
        "ffmpeg: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

#[test]
fn hard_cuts_detected_and_slow_fade_ignored() -> Result<()> {
    let dir = directory("cuts")?;
    let path = dir.join("cuts.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=25:d=1",
            "-f",
            "lavfi",
            "-i",
            "color=blue:s=160x90:r=25:d=1",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=25:d=3",
            "-filter_complex",
            "[2:v]fade=t=out:st=1:d=1[f];[0:v][1:v][f]concat=n=3:v=1:a=0[v]",
            "-map",
            "[v]",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-threads",
            "1",
        ],
        &path,
    )?;
    let asset = probe(&path, "cuts".into())?;
    let cuts = scene_cuts(&asset, SceneParams::default())?;
    assert_eq!(cuts.len(), 2, "{cuts:?}");
    assert_eq!(cuts[0].time_us, 1_000_000);
    assert_eq!(cuts[1].time_us, 2_000_000);
    assert!(cuts.iter().all(|c| c.score >= 0.18 && c.score <= 1.0));
    assert_eq!(
        scene_cuts(
            &asset,
            SceneParams {
                min_gap_us: 2_000_000,
                ..Default::default()
            }
        )?
        .len(),
        1
    );
    Ok(())
}

#[test]
fn rotation_and_variable_frame_pts_are_preserved() -> Result<()> {
    let dir = directory("vfr")?;
    let source = dir.join("source.mp4");
    let path = dir.join("rotated.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=160x90:r=25:d=1",
            "-f",
            "lavfi",
            "-i",
            "color=blue:s=160x90:r=25:d=1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0,settb=1/1000000,setpts='if(lt(N,25),N*40000,1080000+(N-25)*80000)'[v]",
            "-map",
            "[v]",
            "-fps_mode",
            "vfr",
            "-enc_time_base",
            "1/1000000",
            "-c:v",
            "libx264",
            "-threads",
            "1",
        ],
        &source,
    )?;
    ffmpeg(
        &[
            "-display_rotation",
            "90",
            "-i",
            source.to_str().context("UTF-8 path")?,
            "-c",
            "copy",
        ],
        &path,
    )?;
    let asset = probe(&path, "vfr".into())?;
    assert_eq!(asset.rotation % 180, 90);
    let cuts = scene_cuts(&asset, SceneParams::default())?;
    assert_eq!(cuts.len(), 1, "{cuts:?}");
    assert!((cuts[0].time_us - 1_080_000).abs() <= 1, "{cuts:?}");
    Ok(())
}

#[test]
fn pcm_cache_audio_api_and_cli_are_consistent() -> Result<()> {
    let dir = directory("pcm")?;
    let path = dir.join("tone.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:duration=3",
            "-af",
            "volume=enable='between(t,1,2)':volume=0",
            "-c:a",
            "pcm_f32le",
        ],
        &path,
    )?;
    let asset = probe(&path, "tone".into())?;
    let levels = loudness(&asset, &dir, 100_000)?;
    assert_eq!(levels.len(), 30);
    assert!((levels[0] + 21.074).abs() < 0.02);
    let gaps = silences(&asset, &dir, SilenceParams::default())?;
    assert_eq!(gaps.len(), 1);
    assert!((gaps[0].start_us - 1_120_000).abs() <= 30_000);
    assert!((gaps[0].end_us - 1_880_000).abs() <= 30_000);
    let speech = speech_segments(&asset, &dir, SilenceParams::default())?;
    assert_eq!(speech.len(), 2);
    assert_eq!(speech[0].end_us, gaps[0].start_us);
    assert_eq!(speech[1].start_us, gaps[0].end_us);
    assert_eq!(speech[1].end_us, 3_000_000);
    let binary = env!("CARGO_BIN_EXE_capopen-analyze");
    let result = Command::new(binary)
        .arg(&path)
        .arg("silences")
        .arg("--cache")
        .arg(&dir)
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Vec<capopen_analysis::Range>>(&result.stdout)?,
        gaps
    );
    let other = dir.join("silence.wav");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo:d=1",
            "-c:a",
            "pcm_f32le",
        ],
        &other,
    )?;
    let result = Command::new(binary)
        .arg(&other)
        .arg("loudness")
        .arg("--cache")
        .arg(&dir)
        .output()?;
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(value["rms_dbfs"], serde_json::json!(vec![-120.0; 10]));
    Ok(())
}
