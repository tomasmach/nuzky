//! `nuzky style` over the real binary: a spoken recording with retakes, a filler, a side remark
//! and a call to action, and a "creator's" cut of it made with FFmpeg that drops those, shortens
//! pauses, zooms every other piece, makes one piece quieter and burns in two-word captions.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::AtomicBool;

use nuzky_analysis::style::Piece;
use nuzky_mcp::style;
use nuzky_session::transcripts::TranscriptStore;
use serde_json::{Value, json};

/// Sentences, whether the creator's cut keeps them, and the silence after them in seconds.
const SCRIPT: &[(&str, bool, f64)] = &[
    ("So today I want to", false, 0.8),
    ("So today I want to show you how I edit my videos.", true, 0.8),
    ("First I record", false, 0.7),
    ("First I record everything in one long take on my phone.", true, 0.9),
    ("Um.", false, 0.6),
    ("Then I remove the", false, 0.8),
    ("Then I remove the pauses and every slip of the tongue.", true, 1.0),
    ("By the way, my cat is sleeping next to me right now.", false, 1.2),
    ("Captions go on top, two words at a time.", true, 0.8),
    ("The zoom makes the important parts stand out.", true, 0.9),
    ("Music is optional and stays quiet under the voice.", true, 0.8),
    ("That is everything for today.", true, 1.0),
    ("Follow me and leave a comment below.", false, 0.5),
];
const ZOOM: f64 = 1.25;

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("starting a tool the style test needs: ffmpeg, ffprobe or espeak-ng");
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
    output
}

fn ffmpeg(args: &[&str]) {
    run(Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args));
}

fn duration(path: &Path) -> f64 {
    let out = run(Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path));
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

struct Fixture {
    dir: PathBuf,
    recording: PathBuf,
    cut: PathBuf,
    /// Recording time of each kept piece, in the cut's order.
    pieces: Vec<(f64, f64)>,
}

/// The recording, and the creator's cut of it.
fn fixture(dir: &Path) -> Fixture {
    let parts = dir.join("parts");
    std::fs::create_dir_all(&parts).unwrap();
    let mut list = String::new();
    let mut pieces = Vec::new();
    let mut t = 0.0;
    for (i, (text, kept, pause)) in SCRIPT.iter().enumerate() {
        let wav = parts.join(format!("s{i}.wav"));
        run(Command::new("espeak-ng").args(["-v", "en-us", "-s", "160", "-w"]).arg(&wav).arg(text));
        let silence = parts.join(format!("p{i}.wav"));
        ffmpeg(&["-f", "lavfi", "-i", &format!("anullsrc=r=22050:cl=mono:d={pause}"), silence.to_str().unwrap()]);
        let spoken = duration(&wav);
        if *kept {
            pieces.push(((t - 0.06f64).max(0.0), t + spoken + 0.1));
        }
        t += spoken + pause;
        list.push_str(&format!("file '{}'\nfile '{}'\n", wav.display(), silence.display()));
    }
    std::fs::write(parts.join("list.txt"), list).unwrap();
    let speech = dir.join("speech.wav");
    ffmpeg(&["-f", "concat", "-safe", "0", "-i", parts.join("list.txt").to_str().unwrap(), speech.to_str().unwrap()]);
    let recording = dir.join("talk.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=540x960:r=30",
        "-i",
        speech.to_str().unwrap(),
        "-shortest",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        recording.to_str().unwrap(),
    ]);
    // Every other piece zoomed in, the third one quieter, then two-word captions every half second.
    let mut graph = String::new();
    for (k, (start, end)) in pieces.iter().enumerate() {
        let zoom = if k % 2 == 1 { format!(",crop=iw/{ZOOM}:ih/{ZOOM},scale=540:960") } else { String::new() };
        let volume = if k == 2 { ",volume=0.5" } else { "" };
        graph.push_str(&format!(
            "[0:v]trim=start={start}:end={end},setpts=PTS-STARTPTS{zoom}[v{k}];[0:a]atrim=start={start}:end={end},asetpts=PTS-STARTPTS{volume}[a{k}];"
        ));
    }
    let inputs: String = (0..pieces.len()).map(|k| format!("[v{k}][a{k}]")).collect();
    let length: f64 = pieces.iter().map(|(s, e)| e - s).sum();
    let mut srt = String::new();
    let stamp = |s: f64| format!("00:00:{:02},{:03}", s as u64, ((s % 1.0) * 1000.0).round() as u64 % 1000);
    let pairs = ["Hello there", "big zoom", "two words", "short cut", "fast pace", "keep going", "nice edit"];
    for i in 0..(length / 0.5) as usize {
        let text = pairs[i % pairs.len()];
        srt.push_str(&format!("{}\n{} --> {}\n{text}\n\n", i + 1, stamp(i as f64 * 0.5), stamp(i as f64 * 0.5 + 0.5)));
    }
    let captions = dir.join("captions.srt");
    std::fs::write(&captions, srt).unwrap();
    graph.push_str(&format!(
        "{inputs}concat=n={}:v=1:a=1[v][a];[v]subtitles={}:force_style='Fontsize=24,Outline=2,Shadow=0,Alignment=5'[out]",
        pieces.len(),
        captions.display()
    ));
    let cut = dir.join("reel.mp4");
    ffmpeg(&[
        "-i",
        recording.to_str().unwrap(),
        "-filter_complex",
        &graph,
        "-map",
        "[out]",
        "-map",
        "[a]",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        cut.to_str().unwrap(),
    ]);
    Fixture { dir: dir.to_path_buf(), recording, cut, pieces }
}

/// The models and stored transcripts of tmp-test, as scripts/check.sh runs tests.
fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp-test/xdg/data")
}

fn nuzky(fixture: &Fixture, args: &[&Path]) -> Output {
    run(Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args(args)
        .env("XDG_DATA_HOME", data())
        .env("XDG_CACHE_HOME", fixture.dir.join("cache")))
}

/// A project with the recording as one main-track clip per piece, or one clip for all of it.
fn project(fixture: &Fixture, name: &str, pieces: Option<&[(f64, f64)]>) -> PathBuf {
    let path = fixture.dir.join(name);
    nuzky(fixture, &[Path::new("new"), &path, &fixture.recording]);
    let mut project: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    if let Some(pieces) = pieces {
        let template = project["tracks"][0]["clips"][0].clone();
        let mut start = 0i64;
        let clips: Vec<Value> = pieces
            .iter()
            .enumerate()
            .map(|(k, (from, to))| {
                let mut clip = template.clone();
                let length = ((to - from) * 1e6) as i64;
                clip["id"] = json!(format!("piece{k}"));
                clip["startUs"] = json!(start);
                clip["durationUs"] = json!(length);
                clip["content"]["sourceInUs"] = json!((from * 1e6) as i64);
                start += length;
                clip
            })
            .collect();
        project["tracks"][0]["clips"] = json!(clips);
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
    }
    path
}

#[test]
#[ignore = "Recognises speech: needs ffmpeg, espeak-ng and the models in tmp-test/xdg/data/nuzky/models (scripts/fixtures.sh)"]
fn learns_the_creators_style_and_scores_cuts_against_it() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = root.join("tmp-test/style-tests").join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = fixture(&dir);

    // Learning the same pair twice gives the same EDIT.md, byte for byte.
    let (first, second) = (dir.join("EDIT.md"), dir.join("again.md"));
    nuzky(
        &fixture,
        &[Path::new("style"), Path::new("learn"), Path::new("--out"), &first, &fixture.recording, &fixture.cut],
    );
    nuzky(
        &fixture,
        &[Path::new("style"), Path::new("learn"), Path::new("--out"), &second, &fixture.recording, &fixture.cut],
    );
    let style = std::fs::read_to_string(&first).unwrap();
    assert_eq!(style, std::fs::read_to_string(&second).unwrap());
    let refused = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args([Path::new("style"), Path::new("learn"), Path::new("--out"), &first, &fixture.recording, &fixture.cut])
        .env("XDG_DATA_HOME", data())
        .output()
        .unwrap();
    assert!(!refused.status.success(), "an existing EDIT.md may hold the creator's own changes");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), style);

    assert!(!style.chars().any(|c| matches!(c, '\u{2010}'..='\u{2015}' | '\u{2212}')), "{style}");
    assert!(
        style.contains("### Restarted sentences\n\nAn attempt at a sentence that the creator then said again: 3 cut"),
        "{style}"
    );
    assert!(style.contains("The creator kept the latest attempt 3 of 3 times."), "{style}");
    assert!(style.contains(&format!("{} cuts,", fixture.pieces.len() - 1)), "{style}");
    let number = |after: &str| -> f64 {
        let rest = &style[style.find(after).unwrap_or_else(|| panic!("no {after:?} in {style}")) + after.len()..];
        rest[..rest.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-').unwrap()].parse().unwrap()
    };
    assert!(number(", two: ") >= 90.0, "{style}");
    assert!(style.contains("| build_captions max_words | 2 |"), "{style}");
    assert!((number("| Base framing | scale ") - 1.0).abs() <= 0.02, "{style}");
    assert!(
        style.contains(&format!("3 instant zoom ins to a median scale of {ZOOM:.2}, 100% of them at a cut")),
        "{style}"
    );
    assert!(style.contains("Closing calls to action, 1 seen"), "{style}");
    let overview = ["# Settings", "# What gets cut", "# Seen too rarely for a rule"];
    for section in style.split("\n#").skip(1).filter(|s| !overview.iter().any(|o| s.starts_with(o))) {
        let examples = section.lines().filter(|l| l.starts_with("- ")).count();
        assert!(examples >= 3, "a rule needs three examples: {section}");
    }

    // A cut of exactly the creator's pieces matches; keeping everything has every creator word
    // but far more besides. The same input always scores the same.
    let score = |project: &Path| -> Value {
        let out =
            nuzky(&fixture, &[Path::new("style"), Path::new("compare"), &fixture.recording, &fixture.cut, project]);
        let again =
            nuzky(&fixture, &[Path::new("style"), Path::new("compare"), &fixture.recording, &fixture.cut, project]);
        assert_eq!(out.stdout, again.stdout);
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["score"].clone()
    };
    let same = score(&project(&fixture, "same.nuzky", Some(&fixture.pieces)));
    assert_eq!((same["recall"].as_f64(), same["precision"].as_f64()), (Some(1.0), Some(1.0)), "{same}");
    let all = score(&project(&fixture, "all.nuzky", None));
    let expected = all["creator_kept"].as_f64().unwrap() / all["words"].as_f64().unwrap();
    assert_eq!(all["recall"], 1.0, "{all}");
    assert!((all["precision"].as_f64().unwrap() - expected).abs() < 1e-9 && expected < 0.85, "{all}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The creator's pieces as an editor exports them to OpenTimelineIO: the camera's picture on V1 and its
/// sound on A1, cut on frames of 30 fps against the file's timecode of 01:00:00:00, the sound counted in
/// samples, a gap after the third piece and a dissolve after the fourth, the finished video as B-roll
/// above and a song that is not on this computer. Returns the file and the pieces it must give.
fn timeline(fixture: &Fixture) -> (PathBuf, Vec<Piece>) {
    const HOUR: i64 = 3600 * 30;
    let frames = |seconds: f64| (seconds * 30.0).round() as i64;
    let us = |frames: i64| (frames as f64 / 30.0 * 1e6).round() as i64;
    let rt = |frames: i64, audio: bool| {
        let (value, rate) = if audio { (frames * 1600, 48000) } else { (frames, 30) };
        json!({"OTIO_SCHEMA": "RationalTime.1", "value": value, "rate": rate})
    };
    let range = |start: i64, duration: i64, audio: bool| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, audio), "duration": rt(duration, audio)});
    let file = |path: &Path, length: i64, audio: bool| {
        json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "target_url": format!("file://{}", path.display()).replace(' ', "%20"),
            "available_range": range(HOUR, length, audio),
        })
    };
    let clip = |path: &Path, length: i64, from: i64, duration: i64, audio: bool| {
        json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": path.file_name().unwrap().to_string_lossy(),
            "source_range": range(HOUR + from, duration, audio),
            "media_references": {"DEFAULT_MEDIA": file(path, length, audio)},
            "active_media_reference_key": "DEFAULT_MEDIA",
        })
    };
    let gap = |duration: i64, audio: bool| json!({"OTIO_SCHEMA": "Gap.1", "source_range": range(0, duration, audio)});
    let length = (duration(&fixture.recording) * 30.0).floor() as i64;
    let (mut picture, mut sound, mut reference) = (Vec::new(), Vec::new(), Vec::new());
    // Each time in the file is rounded to the microsecond on its own, so the reference adds them up so.
    let mut at_us = 0;
    for (k, (start, end)) in fixture.pieces.iter().enumerate() {
        let (from, duration) = (frames(*start), frames(end - start));
        picture.push(clip(&fixture.recording, length, from, duration, false));
        sound.push(clip(&fixture.recording, length, from, duration, true));
        reference.push(Piece { start_us: at_us, end_us: at_us + us(duration), offset_us: us(from) - at_us });
        at_us += us(duration);
        if k == 2 {
            picture.push(gap(15, false));
            sound.push(gap(15, true));
            at_us += us(15);
        }
        if k == 3 {
            picture.push(json!({
                "OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve",
                "in_offset": rt(6, false), "out_offset": rt(6, false),
            }));
        }
    }
    let track = |kind: &str, children: Vec<Value>| json!({"OTIO_SCHEMA": "Track.1", "name": kind, "kind": kind, "children": children});
    let broll = clip(&fixture.cut, frames(duration(&fixture.cut)), 0, 60, false);
    let song = clip(&fixture.dir.join("not here/song.wav"), 3000, 0, 300, true);
    let otio = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": "Reel",
        "global_start_time": rt(HOUR, false),
        "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
            track("Video", picture),
            track("Video", vec![gap(30, false), broll]),
            track("Audio", sound),
            track("Audio", vec![song]),
        ]},
    });
    let path = fixture.dir.join("Reel.otio");
    std::fs::write(&path, serde_json::to_vec_pretty(&otio).unwrap()).unwrap();
    (path, reference)
}

#[test]
#[ignore = "Recognises speech: needs ffmpeg, espeak-ng and the models in tmp-test/xdg/data/nuzky/models (scripts/fixtures.sh)"]
fn learns_from_a_timeline_cut_in_another_editor() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = root.join("tmp-test/style-tests").join(format!("otio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = fixture(&dir);
    let (otio, reference) = timeline(&fixture);
    let learn = |out: &Path, more: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nuzky"))
            .args(["style", "learn", "--from"])
            .arg(&otio)
            .arg("--out")
            .arg(out)
            .args(more)
            .env("XDG_DATA_HOME", data())
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .output()
            .unwrap()
    };
    let (first, second) = (dir.join("EDIT.md"), dir.join("again.md"));
    let out = learn(&first, &[]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("Learning from talk.mp4 as Reel cut it") && said.contains("Not learning from song.wav"),
        "{said}"
    );
    assert!(!said.contains("reel.mp4"), "B-roll above the main track is not heard: {said}");
    assert!(learn(&second, &[]).status.success());
    let style = std::fs::read_to_string(&first).unwrap();
    assert_eq!(style, std::fs::read_to_string(&second).unwrap(), "the same timeline teaches the same");
    assert!(!learn(&first, &[]).status.success(), "an existing EDIT.md may hold the creator's own changes");
    assert!(learn(&first, &["--replace"]).status.success());
    assert_eq!(std::fs::read_to_string(&first).unwrap(), style);

    assert!(style.contains("| talk.mp4 | en |") && style.contains("| Reel |"), "{style}");
    assert!(
        style.contains("### Restarted sentences\n\nAn attempt at a sentence that the creator then said again: 3 cut"),
        "{style}"
    );
    assert!(style.contains(&format!("{} cuts,", fixture.pieces.len() - 1)), "{style}");
    assert!(style.contains("Captions and zoom not measured in Reel"), "the timeline does not say them: {style}");

    // The pieces are the ones cut, to the microsecond, read again from the words the CLI stored.
    let store = TranscriptStore::at(data().join("nuzky/transcripts")).unwrap();
    let cache = dir.join("cache");
    let taught = style::timeline_lessons(&otio, &store, &|| false, |path| {
        style::video(path, "auto", &store, &cache, &AtomicBool::new(false), |_| panic!("the words are stored"))
    })
    .unwrap();
    let names: Vec<&str> = taught.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["talk.mp4", "song.wav"]);
    let evidence = taught[0].1.as_ref().unwrap();
    assert_eq!(evidence.alignment.pieces, reference);
    assert_eq!(evidence.alignment.matched, 1.0);
    assert_eq!(taught[1].1.as_ref().unwrap_err(), "not on this computer");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_timeline_of_nothing_on_this_computer_teaches_nothing() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = root.join("tmp-test/style-tests").join(format!("otio-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let clip = |url: &str| {
        json!({
            "OTIO_SCHEMA": "Clip.2", "name": "talk",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 30},
                "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": 90, "rate": 30}},
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": url}},
            "active_media_reference_key": "DEFAULT_MEDIA",
        })
    };
    let otio = dir.join("shared.otio");
    let tracks = json!([{"OTIO_SCHEMA": "Track.1", "kind": "Video", "children": [clip("https://example.com/talk.mov"), clip("gone/talk.mov")]}]);
    let file = json!({"OTIO_SCHEMA": "Timeline.1", "name": "Shared", "tracks": {"OTIO_SCHEMA": "Stack.1", "children": tracks}});
    std::fs::write(&otio, file.to_string()).unwrap();
    let premiere = dir.join("edit.prproj");
    std::fs::write(&premiere, "<xml/>").unwrap();
    let learn = |timeline: &Path| {
        Command::new(env!("CARGO_BIN_EXE_nuzky"))
            .args(["style", "learn", "--from"])
            .arg(timeline)
            .arg("--out")
            .arg(dir.join("EDIT.md"))
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .output()
            .unwrap()
    };
    let out = learn(&otio);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{said}");
    assert!(
        said.contains("Not read in")
            && said.contains("Media that is not a file on this computer: https://example.com/talk.mov"),
        "{said}"
    );
    assert!(said.contains("Not learning from talk.mov") && said.contains("Nothing to learn from"), "{said}");
    // A pipe named as a recording is never opened: reading it would wait for ever.
    let pipe = dir.join("pipe.mov");
    assert!(Command::new("mkfifo").arg(&pipe).status().unwrap().success());
    let piped = file.to_string().replace("https://example.com/talk.mov", pipe.to_str().unwrap());
    std::fs::write(&otio, piped).unwrap();
    let out = learn(&otio);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Not learning from pipe.mov in"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = learn(&premiere);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is not a timeline Nuzky reads"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join("EDIT.md").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_sound_file_is_refused_before_any_recognition() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = root.join("tmp-test/style-tests").join(format!("audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sound = dir.join("memo.m4a");
    ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:a", "aac", sound.to_str().unwrap()]);
    let out = Command::new(env!("CARGO_BIN_EXE_nuzky"))
        .args(["style", "compare"])
        .args([&sound, &sound, &dir.join("project.nuzky")])
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("has no picture"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
