//! OpenTimelineIO's own JSON (.otio), read with serde alone: Timeline.1, Stack.1, Track.1, Clip.1 and
//! Clip.2, Gap.1, Transition.1, RationalTime.1, TimeRange.1, ExternalReference.1, Marker.1 and
//! Marker.2, and the time warps LinearTimeWarp.1 and FreezeFrame.1. OTIO has no standard for framing,
//! volume or titles; a clip from a GeneratorReference with a `text` parameter is the only text read.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::{ImportedTimeline, Marker, MediaClip, Read, TextClip, Track, TrackKind, Transition, local_path};

/// Times further from zero than this, eleven days, are no timeline's.
const MAX_US: f64 = 1e12;

#[derive(Clone, Copy, Debug, Deserialize)]
struct RationalTime {
    value: f64,
    rate: f64,
}

impl RationalTime {
    /// Every time carries its own rate: 24, 30000/1001 and 48000 can meet in one file.
    fn us(self) -> Result<i64> {
        ensure!(self.rate.is_finite() && self.rate > 0.0, "a time has the rate {}", self.rate);
        let us = (self.value / self.rate * 1e6).round();
        ensure!(
            us.is_finite() && us.abs() <= MAX_US,
            "the time {} at {} per second is out of range",
            self.value,
            self.rate
        );
        Ok(us as i64)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct TimeRange {
    start_time: RationalTime,
    duration: RationalTime,
}

impl TimeRange {
    fn us(self) -> Result<(i64, i64)> {
        let duration = self.duration.us()?;
        ensure!(duration >= 0, "a range lasts {duration} µs");
        Ok((self.start_time.us()?, duration))
    }
}

#[derive(Deserialize)]
struct Timeline {
    #[serde(default)]
    name: Option<String>,
    tracks: Value,
    #[serde(default)]
    global_start_time: Option<RationalTime>,
}

/// A Stack or a Track: what both hold.
#[derive(Deserialize)]
struct Composition {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    children: Vec<Value>,
    #[serde(default = "on")]
    enabled: bool,
    #[serde(default)]
    source_range: Option<TimeRange>,
    #[serde(default)]
    markers: Vec<Value>,
    #[serde(default)]
    effects: Vec<Value>,
}

#[derive(Deserialize)]
struct Clip {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    source_range: Option<TimeRange>,
    /// Clip.1
    #[serde(default)]
    media_reference: Option<Value>,
    /// Clip.2
    #[serde(default)]
    media_references: BTreeMap<String, Value>,
    #[serde(default)]
    active_media_reference_key: Option<String>,
    #[serde(default)]
    effects: Vec<Value>,
    #[serde(default)]
    markers: Vec<Value>,
    #[serde(default = "on")]
    enabled: bool,
}

#[derive(Deserialize)]
struct Reference {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    available_range: Option<TimeRange>,
    #[serde(default)]
    generator_kind: Option<String>,
    #[serde(default)]
    parameters: Value,
}

#[derive(Deserialize)]
struct Gap {
    #[serde(default)]
    source_range: Option<TimeRange>,
}

#[derive(Deserialize)]
struct OtioTransition {
    #[serde(default)]
    transition_type: Option<String>,
    in_offset: RationalTime,
    out_offset: RationalTime,
}

#[derive(Deserialize)]
struct OtioMarker {
    #[serde(default)]
    name: Option<String>,
    marked_range: TimeRange,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    comment: Option<String>,
}

#[derive(Deserialize)]
struct Effect {
    #[serde(default)]
    time_scalar: Option<f64>,
    #[serde(default = "on")]
    enabled: bool,
}

fn on() -> bool {
    true
}

/// The schema of an OTIO object without its version: "Clip" for "Clip.2".
fn schema(value: &Value) -> &str {
    let tag = value.get("OTIO_SCHEMA").and_then(Value::as_str).unwrap_or("");
    tag.split_once('.').map_or(tag, |(name, _)| name)
}

fn parse<'a, T: Deserialize<'a>>(value: &'a Value) -> Result<T> {
    Ok(T::deserialize(value)?)
}

/// What was not read, counted by kind.
#[derive(Default)]
struct Unread(BTreeMap<String, usize>);

impl Unread {
    fn add(&mut self, what: impl Into<String>) {
        *self.0.entry(what.into()).or_default() += 1;
    }

    fn lines(self) -> Vec<String> {
        self.0.into_iter().map(|(what, n)| if n == 1 { what } else { format!("{what} ({n} times)") }).collect()
    }
}

/// Reads an .otio file's bytes; relative media paths are from `folder`, the file's own.
pub fn read(bytes: &[u8], folder: &Path) -> Result<Read> {
    let root: Value = serde_json::from_slice(bytes).context("It is not an OpenTimelineIO file")?;
    let found = root.get("OTIO_SCHEMA").and_then(Value::as_str).unwrap_or("nothing");
    ensure!(schema(&root) == "Timeline", "It holds {found} instead of a timeline");
    let timeline: Timeline = parse(&root).context("Its timeline is broken")?;
    let stack: Composition = parse(&timeline.tracks).context("Its tracks are broken")?;
    let mut unread = Unread::default();
    let mut out = ImportedTimeline {
        name: timeline.name.unwrap_or_default(),
        fps: timeline.global_start_time.map(|t| t.rate).filter(|r| r.is_finite() && *r > 0.0),
        ..ImportedTimeline::default()
    };
    if stack.source_range.is_some() {
        unread.add("The timeline's own in and out points");
    }
    effects(&stack.effects, &mut unread);
    markers(&stack.markers, 0, 0, 1.0, &mut out.markers, &mut unread);
    for child in &stack.children {
        match schema(child) {
            "Track" => {
                let name = child.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                if let Err(error) = track(child, folder, &mut out, &mut unread) {
                    unread.add(format!("Track {name}: {error:#}"));
                }
            }
            "Stack" => unread.add("A sequence nested in the timeline"),
            other => {
                unread.add(format!("{} in the timeline", if other.is_empty() { "Something unknown" } else { other }))
            }
        }
    }
    if out.fps.is_none() {
        out.fps = first_rate(&stack.children);
    }
    Ok(Read { timeline: out, unread: unread.lines() })
}

/// The rate of the first clip's length, for a timeline that does not say its own.
fn first_rate(tracks: &[Value]) -> Option<f64> {
    tracks
        .iter()
        .flat_map(|t| t.get("children").and_then(Value::as_array).into_iter().flatten())
        .filter(|c| schema(c) == "Clip")
        .find_map(|c| parse::<Clip>(c).ok()?.source_range.map(|r| r.duration.rate))
        .filter(|r| r.is_finite() && *r > 0.0)
}

/// Reads one track into `out`. A part it cannot place in time ends the track there: everything after
/// it would land in the wrong place.
fn track(value: &Value, folder: &Path, out: &mut ImportedTimeline, unread: &mut Unread) -> Result<()> {
    let track: Composition = parse(value)?;
    let kind = match track.kind.as_deref() {
        Some("Video") => TrackKind::Video,
        Some("Audio") => TrackKind::Audio,
        other => {
            unread.add(format!("A track of kind {}", other.unwrap_or("none")));
            return Ok(());
        }
    };
    // A track turned off is not in the cut.
    if !track.enabled {
        return Ok(());
    }
    if track.source_range.is_some() {
        unread.add("A track's own in and out points");
    }
    effects(&track.effects, unread);
    markers(&track.markers, 0, 0, 1.0, &mut out.markers, unread);
    let mut read = Track { name: track.name.unwrap_or_default(), kind, clips: Vec::new(), texts: Vec::new() };
    let mut start_us = 0;
    let mut transition: Option<Transition> = None;
    for child in &track.children {
        match schema(child) {
            "Clip" => {
                let clip: Clip = parse(child)?;
                let name = clip.name.clone().unwrap_or_default();
                let duration = clip_item(clip, start_us, folder, &mut read, &mut out.markers, unread)
                    .with_context(|| format!("clip {name}"))?;
                if let Some(t) = transition.take() {
                    match read.clips.last_mut().filter(|c| c.start_us == start_us) {
                        Some(clip) => clip.transition_in = Some(t),
                        None => unread.add("A transition into something other than a media clip"),
                    }
                }
                start_us += duration;
            }
            "Gap" => {
                let gap: Gap = parse(child)?;
                start_us += gap.source_range.context("a gap has no length")?.us()?.1;
                if transition.take().is_some() {
                    unread.add("A transition into a gap");
                }
            }
            // A transition takes no time: it overlaps the items on either side of the cut.
            "Transition" => {
                let t: OtioTransition = parse(child)?;
                transition = Some(Transition {
                    kind: t.transition_type.unwrap_or_default(),
                    before_us: t.in_offset.us()?,
                    after_us: t.out_offset.us()?,
                });
            }
            other => {
                // Nested sequences and anything unknown still take their time on the track.
                start_us += length(child).with_context(|| format!("{other} has no length Nuzky can read"))?;
                unread.add(match other {
                    "Stack" => "A nested sequence".to_owned(),
                    "" => "Something unknown on a track".to_owned(),
                    other => format!("{other} on a track"),
                });
            }
        }
    }
    if transition.is_some() {
        unread.add("A transition at the end of a track");
    }
    out.tracks.push(read);
    Ok(())
}

/// How long an item lasts on its track.
fn length(value: &Value) -> Result<i64> {
    if let Some(range) = value.get("source_range").filter(|r| !r.is_null()) {
        return Ok(parse::<TimeRange>(range)?.us()?.1);
    }
    let children = value.get("children").and_then(Value::as_array).context("it has no length")?;
    let lengths = children.iter().filter(|c| schema(c) != "Transition").map(length);
    Ok(match schema(value) {
        "Track" => lengths.sum::<Result<i64>>()?,
        _ => lengths.collect::<Result<Vec<i64>>>()?.into_iter().max().unwrap_or(0),
    })
}

/// Reads a clip that starts at `start_us` and returns how long it lasts on the track.
fn clip_item(
    clip: Clip,
    start_us: i64,
    folder: &Path,
    track: &mut Track,
    markers_out: &mut Vec<Marker>,
    unread: &mut Unread,
) -> Result<i64> {
    let name = clip.name.unwrap_or_default();
    let key = clip.active_media_reference_key.as_deref().unwrap_or("DEFAULT_MEDIA");
    let reference = clip.media_reference.as_ref().or_else(|| clip.media_references.get(key)).filter(|r| !r.is_null());
    let parsed: Option<Reference> = reference.map(parse).transpose()?;
    let available = parsed.as_ref().and_then(|r| r.available_range);
    let (source_start, duration_us) = clip.source_range.or(available).context("it has no length")?.us()?;
    if !clip.enabled {
        return Ok(duration_us);
    }
    // A time warp maps the clip's time on the track to its media's; the track time stays the same.
    let mut speed = 1.0;
    for effect in &clip.effects {
        let warp = matches!(schema(effect), "LinearTimeWarp" | "FreezeFrame");
        match parse::<Effect>(effect) {
            Ok(e) if !e.enabled => {}
            Ok(e) if warp => speed *= if schema(effect) == "FreezeFrame" { 0.0 } else { e.time_scalar.unwrap_or(1.0) },
            _ => effects(std::slice::from_ref(effect), unread),
        }
    }
    ensure!(speed.is_finite(), "its speed is {speed}");
    markers(&clip.markers, start_us, source_start, speed, markers_out, unread);
    let Some(media_reference) = parsed else {
        unread.add("A clip without media");
        return Ok(duration_us);
    };
    match media_reference.parameters.get("text").and_then(Value::as_str) {
        Some(text) if media_reference.generator_kind.is_some() => {
            track.texts.push(TextClip { text: text.to_owned(), start_us, duration_us, transform: None });
            return Ok(duration_us);
        }
        _ => {}
    }
    let kind = reference.map_or("", schema);
    if kind != "ExternalReference" {
        unread.add(match kind {
            "MissingReference" | "" => "A clip without media".to_owned(),
            "GeneratorReference" => {
                format!("A generated clip ({})", media_reference.generator_kind.unwrap_or_default())
            }
            "ImageSequenceReference" => "An image sequence".to_owned(),
            other => format!("Media of the kind {other}"),
        });
        return Ok(duration_us);
    }
    let media = media_reference.target_url.unwrap_or_default();
    let path = local_path(&media, folder);
    if path.is_none() {
        unread.add(format!("Media that is not a file on this computer: {}", media_reference.name.unwrap_or(media)));
        return Ok(duration_us);
    }
    if speed < 0.0 {
        unread.add(format!("A clip played backwards: {name}"));
        return Ok(duration_us);
    }
    // Media with a timecode counts its time from there, as its available range starts; the file from 0.
    let source_in_us = source_start - available.map(|r| r.start_time.us()).transpose()?.unwrap_or(0);
    ensure!(source_in_us >= 0, "it starts before its media");
    let source_out_us = source_in_us + (duration_us as f64 * speed).round() as i64;
    if let Some((_, length)) = available.map(TimeRange::us).transpose()? {
        ensure!(source_out_us <= length + 1_000, "it plays past the end of its media");
    }
    track.clips.push(MediaClip {
        name,
        media,
        path,
        source_in_us,
        source_out_us,
        start_us,
        duration_us,
        speed,
        volume: None,
        transform: None,
        keyframes: Vec::new(),
        transition_in: None,
    });
    Ok(duration_us)
}

fn effects(effects: &[Value], unread: &mut Unread) {
    for effect in effects {
        let name = effect.get("effect_name").and_then(Value::as_str).filter(|n| !n.is_empty());
        unread.add(format!("The effect {}", name.unwrap_or(schema(effect))));
    }
}

/// Markers of an item whose time `item_start` shows at `start_us` on the timeline, played at `speed`.
fn markers(values: &[Value], start_us: i64, item_start: i64, speed: f64, out: &mut Vec<Marker>, unread: &mut Unread) {
    for value in values {
        let read = || -> Result<Marker> {
            let marker: OtioMarker = parse(value)?;
            let (at, duration) = marker.marked_range.us()?;
            let scale = if speed > 0.0 { speed } else { f64::INFINITY };
            Ok(Marker {
                name: marker.name.unwrap_or_default(),
                start_us: start_us + ((at - item_start) as f64 / scale).round() as i64,
                duration_us: (duration as f64 / scale).round() as i64,
                color: marker.color.unwrap_or_default(),
                comment: marker.comment.unwrap_or_default(),
            })
        };
        match read() {
            Ok(marker) => out.push(marker),
            Err(_) => unread.add("A marker"),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rt(value: f64, rate: f64) -> Value {
        json!({"OTIO_SCHEMA": "RationalTime.1", "value": value, "rate": rate})
    }

    fn range(start: f64, duration: f64, rate: f64) -> Value {
        json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, rate), "duration": rt(duration, rate)})
    }

    fn clip(name: &str, url: &str, start: f64, duration: f64, rate: f64) -> Value {
        json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": name,
            "source_range": range(start, duration, rate),
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": url}},
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": [],
            "markers": [],
            "enabled": true,
        })
    }

    fn gap(duration: f64, rate: f64) -> Value {
        json!({"OTIO_SCHEMA": "Gap.1", "source_range": range(0.0, duration, rate)})
    }

    fn timeline(tracks: Vec<Value>) -> Value {
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Reel",
            "global_start_time": rt(86400.0, 24.0),
            "tracks": {"OTIO_SCHEMA": "Stack.1", "children": tracks},
        })
    }

    fn track(kind: &str, children: Vec<Value>) -> Value {
        json!({"OTIO_SCHEMA": "Track.1", "name": kind, "kind": kind, "children": children})
    }

    fn read_value(value: Value) -> Result<Read> {
        read(&serde_json::to_vec(&value).unwrap(), Path::new("/edits"))
    }

    #[test]
    fn rational_times_become_microseconds_at_their_own_rate() {
        let at = |value, rate| RationalTime { value, rate }.us().unwrap();
        assert_eq!(at(48.0, 24.0), 2_000_000);
        assert_eq!(at(1.0, 30.0), 33_333);
        assert_eq!(at(2.0, 30.0), 66_667);
        // 23.976 and 29.97 fps: a frame lasts 1001/24000 and 1001/30000 s.
        assert_eq!(at(1.0, 24000.0 / 1001.0), 41_708);
        assert_eq!(at(30.0, 30000.0 / 1001.0), 1_001_000);
        assert_eq!(at(48000.0, 48000.0), 1_000_000);
        assert_eq!(at(-12.0, 24.0), -500_000);
        assert_eq!(at(0.5, 25.0), 20_000, "a time between frames stays where it is");
        for (value, rate) in
            [(1.0, 0.0), (1.0, -24.0), (1e300, 24.0), (1.0, 1e-300), (f64::NAN, 24.0), (1.0, f64::INFINITY)]
        {
            assert!(RationalTime { value, rate }.us().is_err(), "{value} at {rate}");
        }
    }

    #[test]
    fn reads_a_hand_written_cut_with_gaps_and_mixed_rates() {
        let read = read_value(timeline(vec![
            track(
                "Video",
                vec![
                    clip("a", "file:///media/talk.mov", 24.0, 48.0, 24.0),
                    gap(12.0, 24.0),
                    // The same recording at 30 fps, from 10 s for 1.5 s.
                    clip("b", "talk.mov", 300.0, 45.0, 30.0),
                    json!({"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve", "in_offset": rt(6.0, 24.0), "out_offset": rt(6.0, 24.0)}),
                    clip("c", "/media/talk.mov", 24000.0 / 1001.0 * 20.0, 24.0, 24000.0 / 1001.0),
                ],
            ),
            track("Audio", vec![clip("music", "file:///media/song.mp3", 0.0, 96.0, 24.0)]),
        ]))
        .unwrap();
        let t = &read.timeline;
        assert_eq!((t.name.as_str(), t.fps), ("Reel", Some(24.0)));
        assert!(read.unread.is_empty(), "{:?}", read.unread);
        let video = &t.tracks[0];
        assert_eq!(video.kind, TrackKind::Video);
        let times: Vec<_> =
            video.clips.iter().map(|c| (c.start_us, c.duration_us, c.source_in_us, c.source_out_us)).collect();
        assert_eq!(
            times,
            [
                (0, 2_000_000, 1_000_000, 3_000_000),
                (2_500_000, 1_500_000, 10_000_000, 11_500_000),
                (4_000_000, 1_001_000, 20_000_000, 21_001_000),
            ]
        );
        assert_eq!(video.clips[0].path.as_deref(), Some(Path::new("/media/talk.mov")));
        assert_eq!(video.clips[1].path.as_deref(), Some(Path::new("/edits/talk.mov")), "relative to the .otio file");
        assert_eq!(
            video.clips[2].transition_in,
            Some(Transition { kind: "SMPTE_Dissolve".into(), before_us: 250_000, after_us: 250_000 })
        );
        assert_eq!(t.tracks[1].kind, TrackKind::Audio);
        assert_eq!(t.duration_us(), 5_001_000);
    }

    #[test]
    fn media_with_a_timecode_counts_from_the_files_start() {
        let mut c = clip("a", "/media/talk.mov", 86400.0 + 48.0, 24.0, 24.0);
        c["media_references"]["DEFAULT_MEDIA"]["available_range"] = range(86400.0, 2400.0, 24.0);
        let read = read_value(timeline(vec![track("Video", vec![c])])).unwrap();
        let clip = &read.timeline.tracks[0].clips[0];
        assert_eq!((clip.source_in_us, clip.source_out_us), (2_000_000, 3_000_000));
    }

    #[test]
    fn speed_changes_how_much_media_a_clip_plays_not_its_length() {
        let mut fast = clip("fast", "/m/a.mov", 0.0, 48.0, 24.0);
        fast["effects"] =
            json!([{"OTIO_SCHEMA": "LinearTimeWarp.1", "effect_name": "LinearTimeWarp", "time_scalar": 1.5}]);
        let mut held = clip("held", "/m/a.mov", 240.0, 24.0, 24.0);
        held["effects"] = json!([{"OTIO_SCHEMA": "FreezeFrame.1", "effect_name": "FreezeFrame", "time_scalar": 0.0}]);
        let mut back = clip("back", "/m/a.mov", 0.0, 24.0, 24.0);
        back["effects"] = json!([{"OTIO_SCHEMA": "LinearTimeWarp.1", "time_scalar": -1.0}]);
        let read =
            read_value(timeline(vec![track("Video", vec![fast, held, back, clip("d", "/m/a.mov", 0.0, 24.0, 24.0)])]))
                .unwrap();
        let clips = &read.timeline.tracks[0].clips;
        let got: Vec<_> =
            clips.iter().map(|c| (c.name.as_str(), c.start_us, c.duration_us, c.source_out_us, c.speed)).collect();
        assert_eq!(
            got,
            [
                ("fast", 0, 2_000_000, 3_000_000, 1.5),
                ("held", 2_000_000, 1_000_000, 10_000_000, 0.0),
                ("d", 4_000_000, 1_000_000, 1_000_000, 1.0)
            ],
            "the clip played backwards is left out and still takes its time"
        );
        assert_eq!(read.unread, ["A clip played backwards: back"]);
    }

    #[test]
    fn what_is_not_read_is_listed_and_keeps_its_time() {
        let mut effected = clip("zoomed", "/m/a.mov", 0.0, 24.0, 24.0);
        effected["effects"] = json!([{"OTIO_SCHEMA": "Effect.1", "effect_name": "Resolve Effect"}]);
        let nested =
            json!({"OTIO_SCHEMA": "Stack.1", "name": "Intro", "children": [track("Video", vec![gap(48.0, 24.0)])]});
        let title = json!({
            "OTIO_SCHEMA": "Clip.2", "name": "Title", "source_range": range(0.0, 24.0, 24.0),
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "GeneratorReference.1", "generator_kind": "Text", "parameters": {"text": "Hello there"}}},
            "active_media_reference_key": "DEFAULT_MEDIA",
        });
        let missing = json!({
            "OTIO_SCHEMA": "Clip.1", "name": "lost", "source_range": range(0.0, 24.0, 24.0),
            "media_reference": {"OTIO_SCHEMA": "MissingReference.1"},
        });
        let web = clip("web", "https://example.com/a.mov", 0.0, 24.0, 24.0);
        let mut off = clip("off", "/m/a.mov", 0.0, 24.0, 24.0);
        off["enabled"] = json!(false);
        let read = read_value(timeline(vec![
            track("Video", vec![effected, nested, title, missing, web, off, clip("last", "/m/a.mov", 0.0, 24.0, 24.0)]),
            json!({"OTIO_SCHEMA": "Track.1", "kind": "Subtitle", "children": []}),
        ]))
        .unwrap();
        let video = &read.timeline.tracks[0];
        let starts: Vec<_> = video.clips.iter().map(|c| (c.name.as_str(), c.start_us)).collect();
        assert_eq!(starts, [("zoomed", 0), ("last", 7_000_000)], "each part keeps its time on the track");
        assert_eq!(
            video.texts,
            [TextClip { text: "Hello there".into(), start_us: 3_000_000, duration_us: 1_000_000, transform: None }]
        );
        assert_eq!(
            read.unread,
            [
                "A clip without media",
                "A nested sequence",
                "A track of kind Subtitle",
                "Media that is not a file on this computer: https://example.com/a.mov",
                "The effect Resolve Effect",
            ]
        );
    }

    #[test]
    fn a_broken_part_ends_its_track_and_never_the_app() {
        let broken = json!({"OTIO_SCHEMA": "Clip.2", "name": "bad", "source_range": range(0.0, 24.0, 0.0)});
        let read = read_value(timeline(vec![
            track(
                "Video",
                vec![clip("ok", "/m/a.mov", 0.0, 24.0, 24.0), broken, clip("after", "/m/a.mov", 0.0, 24.0, 24.0)],
            ),
            track("Audio", vec![clip("sound", "/m/a.mov", 0.0, 24.0, 24.0)]),
        ]))
        .unwrap();
        assert_eq!(read.unread, ["Track Video: clip bad: a time has the rate 0"]);
        assert!(read.timeline.tracks.iter().all(|t| t.kind == TrackKind::Audio), "the broken track is left out whole");
        for bad in [
            b"not json".as_slice(),
            br#"{"OTIO_SCHEMA": "SerializableCollection.1", "children": []}"#,
            br#"{"OTIO_SCHEMA": "Timeline.1"}"#,
            br#"{"OTIO_SCHEMA": "Timeline.1", "tracks": 5}"#,
            br#"[1, 2]"#,
        ] {
            assert!(super::read(bad, Path::new("/")).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        let deep = "[".repeat(100_000);
        assert!(
            super::read(deep.as_bytes(), Path::new("/")).is_err(),
            "deep nesting is an error, not a stack overflow"
        );
    }

    #[test]
    fn markers_land_on_the_timeline() {
        let mut c = clip("a", "/m/a.mov", 240.0, 48.0, 24.0);
        c["markers"] = json!([{"OTIO_SCHEMA": "Marker.2", "name": "hook", "marked_range": range(252.0, 0.0, 24.0), "color": "RED", "comment": "best line"}]);
        let mut t = timeline(vec![track("Video", vec![gap(24.0, 24.0), c])]);
        t["tracks"]["markers"] = json!([{"OTIO_SCHEMA": "Marker.1", "name": "end", "marked_range": range(72.0, 0.0, 24.0), "color": "GREEN"}]);
        let read = read_value(t).unwrap();
        let at: Vec<_> =
            read.timeline.markers.iter().map(|m| (m.name.as_str(), m.start_us, m.comment.as_str())).collect();
        assert_eq!(at, [("end", 3_000_000, ""), ("hook", 1_500_000, "best line")]);
    }
}
