//! Timelines cut in other editors, read into one neutral model, whichever editor made them. Each
//! format reads its files into an `ImportedTimeline` and says in plain words what it did not read.
//! A timeline file comes from anyone: reading opens only that file, never the media it names nor
//! the network, and a broken file is an error, never a crash.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

pub mod otio;

/// A larger file is not a timeline.
const MAX_FILE: u64 = 64 << 20;

/// A timeline as another editor cut it. Times are microseconds from the timeline's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedTimeline {
    pub name: String,
    /// Frames per second, when the file says.
    pub fps: Option<f64>,
    /// Width and height in pixels, when the file says.
    pub size: Option<(u32, u32)>,
    /// Bottom to top: the lowest video track is the main one, those above lie over it.
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
}

impl ImportedTimeline {
    pub fn duration_us(&self) -> i64 {
        let ends = self.tracks.iter().flat_map(|t| {
            t.clips.iter().map(MediaClip::end_us).chain(t.texts.iter().map(|c| c.start_us + c.duration_us))
        });
        ends.max().unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    /// In timeline order.
    pub clips: Vec<MediaClip>,
    pub texts: Vec<TextClip>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaClip {
    pub name: String,
    /// The media as the file names it: a path or an address.
    pub media: String,
    /// The local file it names, a relative path from the timeline file's folder; none for anything
    /// else, such as a web address.
    pub path: Option<PathBuf>,
    /// From the start of the media file.
    pub source_in_us: i64,
    pub source_out_us: i64,
    pub start_us: i64,
    pub duration_us: i64,
    /// Media time per timeline time: 2 plays twice as fast, 0 holds a frame.
    pub speed: f64,
    /// Linear gain, when the file says.
    pub volume: Option<f32>,
    /// Where the picture sits, when the file says.
    pub transform: Option<Transform>,
    /// From the clip's start.
    pub keyframes: Vec<Keyframe>,
    /// Into this clip from the one before it, or from nothing.
    pub transition_in: Option<Transition>,
}

impl MediaClip {
    pub fn end_us(&self) -> i64 {
        self.start_us + self.duration_us
    }
}

/// As Nuzky has it: scale 1 fits the frame, x and y move the picture by fractions of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframe {
    pub at_us: i64,
    pub transform: Transform,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// As the editor calls it, such as SMPTE_Dissolve.
    pub kind: String,
    /// How long it plays before the cut and after it.
    pub before_us: i64,
    pub after_us: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextClip {
    pub text: String,
    pub start_us: i64,
    pub duration_us: i64,
    pub transform: Option<Transform>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub start_us: i64,
    pub duration_us: i64,
    pub color: String,
    pub comment: String,
}

/// A timeline file as read: the timeline, and one line for each kind of thing in it that was not read.
#[derive(Clone, Debug)]
pub struct Read {
    pub timeline: ImportedTimeline,
    pub unread: Vec<String>,
}

/// Reads the timeline file at `path`, by its extension.
pub fn read(path: &Path) -> Result<Read> {
    let meta = std::fs::metadata(path).with_context(|| format!("Cannot find {}", path.display()))?;
    ensure!(meta.is_file(), "{} is not a file", path.display());
    ensure!(meta.len() <= MAX_FILE, "{} is too large for a timeline", path.display());
    let bytes = std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let folder = std::path::absolute(path)?.parent().map(Path::to_path_buf).unwrap_or_default();
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("otio") => otio::read(&bytes, &folder),
        _ => bail!(
            "{} is not a timeline Nuzky reads. Export the timeline from your editor as OpenTimelineIO (.otio).",
            path.display()
        ),
    }
}

/// The local file a media reference names, a relative one from `folder`. Any other address, a
/// web one above all, names nothing Nuzky opens.
pub fn local_path(target: &str, folder: &Path) -> Option<PathBuf> {
    // One letter before the colon is a Windows drive, not a scheme.
    let scheme = target
        .split_once(':')
        .map(|(s, _)| s)
        .filter(|s| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    let path = match scheme {
        Some(s) if s.eq_ignore_ascii_case("file") => {
            let rest = &target[s.len() + 1..];
            let rest = rest.strip_prefix("//").map_or(rest, |r| r.strip_prefix("localhost").unwrap_or(r));
            // file://host/… is a file on another computer.
            if !rest.starts_with('/') {
                return None;
            }
            PathBuf::from(percent_encoding::percent_decode_str(rest).decode_utf8().ok()?.as_ref())
        }
        Some(_) => return None,
        None if target.is_empty() => return None,
        None => PathBuf::from(target),
    };
    Some(if path.is_absolute() { path } else { folder.join(path) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_files_are_media() {
        let folder = Path::new("/home/me/edits");
        assert_eq!(local_path("file:///home/me/My%20Talk.mov", folder), Some("/home/me/My Talk.mov".into()));
        assert_eq!(local_path("file://localhost/home/me/a.mov", folder), Some("/home/me/a.mov".into()));
        assert_eq!(local_path("FILE:/home/me/a.mov", folder), Some("/home/me/a.mov".into()));
        assert_eq!(local_path("media/a.mov", folder), Some("/home/me/edits/media/a.mov".into()));
        assert_eq!(local_path("/abs/a.mov", folder), Some("/abs/a.mov".into()));
        for foreign in
            ["http://example.com/a.mov", "https://x/a.mov", "smb://nas/a.mov", "file://nas/a.mov", "ftp:a", ""]
        {
            assert_eq!(local_path(foreign, folder), None, "{foreign}");
        }
    }
}
