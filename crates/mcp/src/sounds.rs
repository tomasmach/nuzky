//! The sound library: sound effects built into Nuzky, and music and sounds found online through
//! Openverse, or Freesound with the user's own key. Only CC0 and CC BY ever show, the licences that
//! allow monetized videos. Searching sends the search text; previewing or adding a sound downloads
//! that one file. Nothing from the project leaves the computer.
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use nuzky_engine::edit::new_id;
use nuzky_engine::model::{Asset, AssetKind, Credit, License};
use nuzky_session::jobs::check_cancel;
use serde::{Deserialize, Serialize};

const OPENVERSE: &str = "https://api.openverse.org/v1/";
const FREESOUND: &str = "https://freesound.org/apiv2/";
/// Wikimedia asks for contact details in the User-Agent.
const AGENT: &str = concat!("Nuzky/", env!("CARGO_PKG_VERSION"), " (https://nuzky.app)");
/// Tests serve both APIs and their files from this host through `NUZKY_OPENVERSE_URL` and
/// `NUZKY_FREESOUND_URL`; nothing else replaces the real services.
const TEST_SERVER: &str = "http://127.0.0.1:";
const PAGE_SIZE: u32 = 20;
const MAX_QUERY_CHARS: usize = 200;
const MAX_JSON_BYTES: u64 = 4 << 20;
/// Longer music than anyone puts under a short video.
const MAX_FILE_BYTES: u64 = 60 << 20;
const CC_BY_VERSIONS: &[&str] = &["1.0", "2.0", "2.5", "3.0", "4.0"];

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "SoundKind"))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Music,
    Effect,
}

/// A sound the library offers.
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sound {
    /// `nuzky:whoosh` for a built-in sound, `openverse:<uuid>` or `freesound:<number>`.
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub author: String,
    /// 0 when the source does not say.
    pub duration_us: i64,
    pub license: License,
    pub license_version: String,
    pub license_url: String,
    /// The sound's page at its source, where its licence can be checked; empty for built-in sounds.
    pub url: String,
    /// "Built in", "Jamendo", "Freesound", "Wikimedia Commons"…
    pub provider: String,
    /// The group of a built-in sound, such as "Whoosh".
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub category: Option<String>,
    /// What gets downloaded, or the built-in file's name.
    #[serde(skip)]
    file: String,
    /// The file's type when its address does not end in it, as Jamendo's do not.
    #[serde(skip)]
    filetype: Option<String>,
}

impl Sound {
    pub fn credit(&self) -> Credit {
        let (source, id) = self.id.split_once(':').unwrap_or(("", &self.id));
        Credit {
            source: source.into(),
            id: id.into(),
            title: self.title.clone(),
            author: self.author.clone(),
            license: self.license,
            license_version: self.license_version.clone(),
            license_url: self.license_url.clone(),
            url: self.url.clone(),
        }
    }
}

/// One page of online results.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "SoundPage"))]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub sounds: Vec<Sound>,
    pub more: bool,
    /// "Openverse" or "Freesound": the service the results come from, which the library names.
    pub service: &'static str,
}

// ---------- Built in ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Packed {
    id: String,
    file: String,
    title: String,
    category: String,
    author: String,
    source: String,
    duration_us: i64,
    /// Checked against the embedded file by a test.
    #[cfg_attr(not(test), allow(dead_code))]
    sha256: String,
}

macro_rules! pack {
    ($($file:literal),* $(,)?) => {
        &[$(($file, include_bytes!(concat!("../../../assets/sounds/", $file)) as &[u8])),*]
    };
}

/// Every file of assets/sounds/manifest.json; a test keeps the two lists the same.
const FILES: &[(&str, &[u8])] = pack![
    "whoosh.ogg",
    "swoosh.ogg",
    "swish-short.ogg",
    "swish-tiny.ogg",
    "swish-tiny-2.ogg",
    "riser-retro.ogg",
    "phaser-sweep.ogg",
    "swipe-in.ogg",
    "swipe-out.ogg",
    "glitch.ogg",
    "glitch-2.ogg",
    "bubble-pop.ogg",
    "click.ogg",
    "click-sharp.ogg",
    "tap.ogg",
    "wood-tap.ogg",
    "key-press.ogg",
    "ding.ogg",
    "bell.ogg",
    "bell-long.ogg",
    "notification.ogg",
    "success.ogg",
    "success-chime.ogg",
    "error.ogg",
    "boom.ogg",
    "punch.ogg",
    "soft-hit.ogg",
    "metal-hit.ogg",
    "crunch.ogg",
    "rimshot.ogg",
    "boing.ogg",
    "slime.ogg",
    "power-up.ogg",
    "record-scratch.ogg",
    "sparkle.ogg",
    "laugh.ogg",
    "applause.ogg",
    "camera-shutter.ogg",
    "cash-register.ogg",
    "coins.ogg",
    "typing.ogg",
    "typing-steady.ogg",
    "heartbeat.ogg",
];

fn packed() -> &'static [Packed] {
    static PACKED: std::sync::OnceLock<Vec<Packed>> = std::sync::OnceLock::new();
    PACKED.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/sounds/manifest.json")).expect("valid sound manifest")
    })
}

/// The built-in sound effects whose title, group or id contains `query`, all of them for an empty one.
pub fn built_in(query: &str) -> Vec<Sound> {
    let query = query.trim().to_lowercase();
    packed()
        .iter()
        .filter(|p| {
            query.is_empty() || [&p.title, &p.category, &p.id].iter().any(|t| t.to_lowercase().contains(&query))
        })
        .map(|p| Sound {
            id: format!("nuzky:{}", p.id),
            kind: Kind::Effect,
            title: p.title.clone(),
            author: p.author.clone(),
            duration_us: p.duration_us,
            license: License::Cc0,
            license_version: "1.0".into(),
            license_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
            url: p.source.clone(),
            provider: "Built in".into(),
            category: Some(p.category.clone()),
            file: p.file.clone(),
            filetype: None,
        })
        .collect()
}

// ---------- Settings ----------

/// Freesound needs the user's own key; it is kept beside the projects, readable only by the user.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub freesound: bool,
    pub freesound_key: String,
}

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nuzky")
}

fn settings_path() -> PathBuf {
    data_dir().join("sound-library.json")
}

pub fn settings() -> Settings {
    std::fs::read_to_string(settings_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let key = settings.freesound_key.trim();
    ensure!(
        key.len() <= 128 && key.chars().all(|c| c.is_ascii_alphanumeric()),
        "INVALID_KEY: a Freesound API key is letters and digits only"
    );
    let path = settings_path();
    std::fs::create_dir_all(data_dir())?;
    let tmp = path.with_extension(format!("{}.tmp", new_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp).context("Saving sound library settings")?;
    let saved = Settings { freesound: settings.freesound, freesound_key: key.into() };
    std::io::Write::write_all(&mut file, serde_json::to_string(&saved)?.as_bytes())?;
    drop(file);
    std::fs::rename(&tmp, &path).context("Saving sound library settings")
}

/// The key, when the user turned Freesound on and gave one.
fn freesound_key() -> Option<String> {
    let settings = settings();
    (settings.freesound && !settings.freesound_key.is_empty()).then_some(settings.freesound_key)
}

// ---------- Network ----------

fn test_base(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|url| url.starts_with(TEST_SERVER))
}

fn testing() -> bool {
    test_base("NUZKY_OPENVERSE_URL").is_some() || test_base("NUZKY_FREESOUND_URL").is_some()
}

/// Only web addresses are fetched, and plain http only from the local test server.
fn fetchable(url: &str) -> bool {
    (url.starts_with("https://") || (testing() && url.starts_with(TEST_SERVER))) && !url.contains(char::is_whitespace)
}

fn agent(body_timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(!testing())
        .max_redirects(5)
        .user_agent(AGENT)
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(body_timeout))
        .build()
        .into()
}

/// The coded, plain error the library shows for a failed request to `service`.
fn request_error(service: &str, error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::StatusCode(429) => {
            anyhow!("RATE_LIMITED: {service} gets too many searches from this network. Try again in a minute.")
        }
        ureq::Error::StatusCode(401) => anyhow!("KEY_REJECTED: {service} did not accept your API key."),
        ureq::Error::StatusCode(403) => anyhow!("SOURCE_REFUSED: {service} refused the request. Try again later."),
        ureq::Error::StatusCode(404) => anyhow!("SOUND_UNAVAILABLE: {service} no longer has this sound."),
        ureq::Error::StatusCode(code) => anyhow!("SOURCE_FAILED: {service} answered with an error ({code})."),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::Timeout(_) => {
            anyhow!("OFFLINE: Can't reach {service}. Check your internet connection.")
        }
        error => anyhow!("SOURCE_FAILED: {service}: {error}"),
    }
}

fn get_json<T: serde::de::DeserializeOwned>(service: &str, url: &str, key: Option<&str>) -> Result<T> {
    let mut call = agent(Duration::from_secs(20)).get(url);
    if let Some(key) = key {
        call = call.header("Authorization", format!("Token {key}"));
    }
    let mut response = call.call().map_err(|e| request_error(service, e))?;
    let mut text = String::new();
    response
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .reader()
        .take(MAX_JSON_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| anyhow!("OFFLINE: Can't reach {service}. Check your internet connection. ({e})"))?;
    serde_json::from_str(&text).with_context(|| format!("SOURCE_FAILED: {service} sent something unexpected"))
}

fn query(pairs: &[(&str, String)]) -> String {
    let encode = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    pairs.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect::<Vec<_>>().join("&")
}

/// Text from a catalogue as one short line, within what a project may hold.
fn line(text: Option<&str>, max: usize) -> String {
    let text: String = text.unwrap_or_default().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The licence of a Creative Commons deed URL, when it is CC0 1.0 or a known CC BY.
fn license_of(url: &str) -> Option<(License, String, String)> {
    let path =
        url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?.strip_prefix("creativecommons.org/")?;
    let path = path.trim_end_matches('/');
    if path == "publicdomain/zero/1.0" {
        return Some((License::Cc0, "1.0".into(), "https://creativecommons.org/publicdomain/zero/1.0/".into()));
    }
    // Ported CC BY licences end in a country, such as licenses/by/3.0/us; keep the exact deed.
    let rest = path.strip_prefix("licenses/by/")?;
    let version = rest.split('/').next()?;
    (CC_BY_VERSIONS.contains(&version)
        && rest.split('/').count() <= 2
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '/'))
    .then(|| (License::CcBy, version.into(), format!("https://creativecommons.org/{path}/")))
}

/// Results that came from a search, so preview and add need no second request.
static SEEN: Mutex<Option<HashMap<String, Sound>>> = Mutex::new(None);

fn remember(sounds: &[Sound]) {
    let mut seen = SEEN.lock().unwrap();
    let seen = seen.get_or_insert_with(HashMap::new);
    if seen.len() > 2000 {
        seen.clear();
    }
    for sound in sounds {
        seen.insert(sound.id.clone(), sound.clone());
    }
}

// ---------- Openverse ----------

#[derive(Deserialize)]
struct OpenverseAudio {
    id: String,
    title: Option<String>,
    creator: Option<String>,
    license: String,
    license_version: Option<String>,
    license_url: Option<String>,
    foreign_landing_url: Option<String>,
    url: Option<String>,
    /// Milliseconds.
    duration: Option<i64>,
    source: Option<String>,
    filetype: Option<String>,
}

#[derive(Deserialize)]
struct OpenversePage {
    results: Vec<OpenverseAudio>,
    page_count: u32,
}

fn openverse_base() -> String {
    test_base("NUZKY_OPENVERSE_URL").unwrap_or_else(|| OPENVERSE.into())
}

fn provider_name(source: &str) -> String {
    match source {
        "jamendo" => "Jamendo".into(),
        "freesound" => "Freesound".into(),
        "wikimedia_audio" => "Wikimedia Commons".into(),
        other => line(Some(other), 40),
    }
}

/// A result the library may show: CC0 1.0 or CC BY, the licence's own deed, a web page to check it
/// on and a file to download, and for CC BY an author to credit.
fn from_openverse(audio: OpenverseAudio, kind: Kind) -> Option<Sound> {
    let (license, version, license_url) = license_of(audio.license_url.as_deref()?)?;
    let expected = match license {
        License::Cc0 => "cc0",
        License::CcBy => "by",
    };
    if audio.license != expected || audio.license_version.as_deref() != Some(version.as_str()) {
        return None;
    }
    let (url, file) = (audio.foreign_landing_url?, audio.url?);
    let author = line(audio.creator.as_deref(), 300);
    if !url.starts_with("https://") || !fetchable(&file) || (license == License::CcBy && author.is_empty()) {
        return None;
    }
    let id = audio.id.to_ascii_lowercase();
    if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    Some(Sound {
        id: format!("openverse:{id}"),
        kind,
        title: Some(line(audio.title.as_deref(), 300)).filter(|t| !t.is_empty()).unwrap_or_else(|| "Untitled".into()),
        author: if author.is_empty() { "Unknown".into() } else { author },
        duration_us: audio.duration.unwrap_or(0).max(0) * 1000,
        license,
        license_version: version,
        license_url,
        url: line(Some(&url), 2048),
        provider: provider_name(audio.source.as_deref().unwrap_or("Openverse")),
        category: None,
        file,
        filetype: audio.filetype,
    })
}

fn openverse(text: &str, kind: Kind, license: Option<License>, page: u32) -> Result<Page> {
    let licenses = match license {
        Some(License::Cc0) => "cc0",
        Some(License::CcBy) => "by",
        None => "cc0,by",
    };
    let mut pairs = vec![
        ("q", text.to_owned()),
        ("license", licenses.to_owned()),
        ("page", page.to_string()),
        ("page_size", PAGE_SIZE.to_string()),
        ("mature", "false".to_owned()),
        ("filter_dead", "true".to_owned()),
    ];
    // Sound effects come from Freesound, where most have no category; music is whatever is filed as music.
    match kind {
        Kind::Music => pairs.push(("category", "music".to_owned())),
        Kind::Effect => pairs.push(("source", "freesound".to_owned())),
    }
    let url = format!("{}audio/?{}", openverse_base(), query(&pairs));
    let found: OpenversePage = get_json("Openverse", &url, None)?;
    let sounds: Vec<Sound> = found.results.into_iter().filter_map(|a| from_openverse(a, kind)).collect();
    remember(&sounds);
    Ok(Page { sounds, more: page < found.page_count, service: "Openverse" })
}

fn openverse_sound(id: &str) -> Result<Sound> {
    let audio: OpenverseAudio = get_json("Openverse", &format!("{}audio/{id}/", openverse_base()), None)?;
    from_openverse(audio, Kind::Effect).context("UNLICENSED: this sound's licence does not allow monetized videos")
}

// ---------- Freesound ----------

#[derive(Deserialize)]
struct FreesoundSound {
    id: u64,
    name: Option<String>,
    username: Option<String>,
    /// Seconds.
    duration: Option<f64>,
    url: Option<String>,
    license: String,
    previews: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct FreesoundPage {
    results: Vec<FreesoundSound>,
    next: Option<String>,
}

fn freesound_base() -> String {
    test_base("NUZKY_FREESOUND_URL").unwrap_or_else(|| FREESOUND.into())
}

const FREESOUND_FIELDS: &str = "id,name,username,duration,url,license,previews";

/// Its previews need no account to download; originals would.
fn from_freesound(sound: FreesoundSound) -> Option<Sound> {
    let (license, license_version, license_url) = license_of(&sound.license)?;
    let url = sound.url.filter(|url| url.starts_with("https://"))?;
    let file = sound.previews?.remove("preview-hq-ogg").filter(|file| fetchable(file))?;
    let author = line(sound.username.as_deref(), 300);
    if author.is_empty() {
        return None;
    }
    Some(Sound {
        id: format!("freesound:{}", sound.id),
        kind: Kind::Effect,
        title: Some(line(sound.name.as_deref(), 300)).filter(|t| !t.is_empty()).unwrap_or_else(|| "Untitled".into()),
        author,
        duration_us: (sound.duration.unwrap_or(0.0).max(0.0) * 1e6) as i64,
        license,
        license_version,
        license_url,
        url: line(Some(&url), 2048),
        provider: "Freesound".into(),
        category: None,
        file,
        filetype: None,
    })
}

fn freesound(key: &str, text: &str, license: Option<License>, page: u32) -> Result<Page> {
    let filter = match license {
        Some(License::Cc0) => r#"license:"Creative Commons 0""#,
        Some(License::CcBy) => r#"license:"Attribution""#,
        None => r#"license:("Creative Commons 0" OR "Attribution")"#,
    };
    let pairs = [
        ("query", text.to_owned()),
        ("filter", filter.to_owned()),
        ("fields", FREESOUND_FIELDS.to_owned()),
        ("page", page.to_string()),
        ("page_size", PAGE_SIZE.to_string()),
    ];
    let found: FreesoundPage =
        get_json("Freesound", &format!("{}search/?{}", freesound_base(), query(&pairs)), Some(key))?;
    let sounds: Vec<Sound> = found.results.into_iter().filter_map(from_freesound).collect();
    remember(&sounds);
    Ok(Page { sounds, more: found.next.is_some(), service: "Freesound" })
}

fn freesound_sound(id: &str) -> Result<Sound> {
    let key = freesound_key().context("KEY_MISSING: turn on Freesound and add your API key in the sound library")?;
    let url = format!("{}sounds/{id}/?fields={FREESOUND_FIELDS}", freesound_base());
    from_freesound(get_json("Freesound", &url, Some(&key))?)
        .context("UNLICENSED: this sound's licence does not allow monetized videos")
}

// ---------- Search and download ----------

/// One page of online results. Sound effects come from Freesound when the user turned it on with a key,
/// otherwise everything comes from Openverse.
pub fn search(text: &str, kind: Kind, license: Option<License>, page: u32) -> Result<Page> {
    let text = text.trim();
    ensure!(!text.is_empty(), "INVALID_ARGUMENTS: type what to search for");
    ensure!(text.chars().count() <= MAX_QUERY_CHARS, "INVALID_ARGUMENTS: the search text is too long");
    ensure!((1..=12).contains(&page), "INVALID_ARGUMENTS: page is 1 to 12");
    match (kind, freesound_key()) {
        (Kind::Effect, Some(key)) => freesound(&key, text, license, page),
        _ => openverse(text, kind, license, page),
    }
}

/// The sound behind an id from `search` or `built_in`, checked again at its source when it was not
/// found in this session.
pub fn sound(id: &str) -> Result<Sound> {
    if let Some(sound) = SEEN.lock().unwrap().as_ref().and_then(|seen| seen.get(id)) {
        return Ok(sound.clone());
    }
    let unknown = || anyhow!("SOUND_UNKNOWN: {id:?} is not a sound id from search_sounds");
    match id.split_once(':').ok_or_else(unknown)? {
        ("nuzky", name) => built_in("").into_iter().find(|s| s.id == id && !name.is_empty()).ok_or_else(unknown),
        ("openverse", uuid) if uuid.len() == 36 && uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') => {
            openverse_sound(uuid)
        }
        ("freesound", number)
            if !number.is_empty() && number.len() < 20 && number.chars().all(|c| c.is_ascii_digit()) =>
        {
            freesound_sound(number)
        }
        _ => Err(unknown()),
    }
}

/// Where downloaded and built-in sounds live once a project uses them: not a cache, since the
/// projects point at these files.
pub fn library_dir() -> PathBuf {
    data_dir().join("sounds")
}

/// A safe file name for the sound, keeping the extension of what was downloaded.
fn file_name(sound: &Sound) -> String {
    let stem: String = sound.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let ext = sound
        .file
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let known = |ext: &String| matches!(ext.as_str(), "mp3" | "ogg" | "opus" | "wav" | "flac" | "m4a" | "aac");
    // Openverse calls Jamendo's MP3s "mp32".
    let named = sound.filetype.as_ref().map(|t| t.to_ascii_lowercase().replace("mp32", "mp3"));
    let ext = ext.filter(known).or(named.filter(known)).unwrap_or("audio".into());
    format!("{stem}.{ext}")
}

/// Writes through a temporary file beside `path`, so a file there is always whole.
fn publish(path: &Path, write: impl FnOnce(&mut std::fs::File) -> Result<()>) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("Sound needs a directory")?)?;
    let tmp = path.with_file_name(format!(".nuzky-part-{}", new_id()));
    let result = std::fs::File::create(&tmp)
        .context("Saving the sound")
        .and_then(|mut file| write(&mut file).and_then(|()| file.sync_all().context("Saving the sound")))
        .and_then(|()| std::fs::rename(&tmp, path).context("Saving the sound"));
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

fn download(sound: &Sound, path: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<()> {
    let service = sound.provider.clone();
    ensure!(fetchable(&sound.file), "SOUND_UNAVAILABLE: {service} gave no file to download");
    let response = agent(Duration::from_secs(30)).get(&sound.file).call().map_err(|e| request_error(&service, e))?;
    let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    ensure!(total.is_none_or(|t| t <= MAX_FILE_BYTES), "SOUND_UNAVAILABLE: this file is too big for a sound");
    publish(path, |file| {
        let mut reader = response.into_body().into_reader();
        let mut buffer = vec![0; 1 << 16];
        let mut done = 0u64;
        loop {
            check_cancel(cancel)?;
            let read =
                reader.read(&mut buffer).map_err(|e| anyhow!("OFFLINE: The download from {service} stopped. ({e})"))?;
            if read == 0 {
                return Ok(());
            }
            done += read as u64;
            ensure!(done <= MAX_FILE_BYTES, "SOUND_UNAVAILABLE: this file is too big for a sound");
            std::io::Write::write_all(file, &buffer[..read])?;
            if let Some(total) = total {
                progress((done as f32 / total.max(1) as f32).min(1.0));
            }
        }
    })
}

/// What the download thread reports.
enum Step {
    Progress(f32),
    Done(Box<Result<(PathBuf, Sound)>>),
}

/// The sound as a local file: a built-in one in the library, a downloaded one in `cache_dir` until
/// a project uses it. A file already there is used again. The network work runs on its own thread, so
/// `cancelled` stops this within 100 ms even while a server keeps silent; the thread then drops its part.
pub fn fetch(
    id: &str,
    cache_dir: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(f32),
) -> Result<(PathBuf, Sound)> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let (id, cache_dir, worker_stop) = (id.to_owned(), cache_dir.to_owned(), stop.clone());
    std::thread::Builder::new().name("sound-download".into()).spawn(move || {
        let steps = tx.clone();
        let result = fetch_here(&id, &cache_dir, &worker_stop, |p| drop(steps.send(Step::Progress(p))));
        tx.send(Step::Done(Box::new(result))).ok();
    })?;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Step::Progress(p)) => progress(p),
            Ok(Step::Done(result)) => return *result,
            Err(RecvTimeoutError::Timeout) if cancelled() => {
                stop.store(true, Ordering::Relaxed);
                bail!("CANCELLED: the download stopped");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("SOURCE_FAILED: the download stopped unexpectedly"),
        }
    }
}

fn fetch_here(id: &str, cache_dir: &Path, cancel: &AtomicBool, progress: impl FnMut(f32)) -> Result<(PathBuf, Sound)> {
    let sound = sound(id)?;
    if sound.provider == "Built in" {
        let path = library_dir().join("nuzky").join(&sound.file);
        let (_, bytes) = FILES.iter().find(|(file, _)| *file == sound.file).context("Built-in sound missing")?;
        if std::fs::read(&path).ok().as_deref() != Some(*bytes) {
            publish(&path, |file| Ok(std::io::Write::write_all(file, bytes)?))?;
        }
        return Ok((path, sound));
    }
    let library = library_dir().join(file_name(&sound));
    if library.is_file() {
        return Ok((library, sound));
    }
    let path = cache_dir.join("sounds").join(file_name(&sound));
    if !path.is_file() {
        download(&sound, &path, cancel, progress)?;
    }
    Ok((path, sound))
}

/// The sound as a project asset, its file moved from the cache into the library first, since a cache
/// may be emptied while the project still needs the file.
pub fn asset(id: &str, cache_dir: &Path, cancelled: impl Fn() -> bool, progress: impl FnMut(f32)) -> Result<Asset> {
    let (fetched, sound) = fetch(id, cache_dir, &cancelled, progress)?;
    ensure!(!cancelled(), "CANCELLED: the download stopped");
    let path = if fetched.starts_with(library_dir()) {
        fetched
    } else {
        let kept = library_dir().join(file_name(&sound));
        if !kept.is_file() {
            publish(&kept, |file| Ok(std::io::copy(&mut std::fs::File::open(&fetched)?, file).map(drop)?))?;
        }
        kept
    };
    let mut asset = nuzky_engine::media::probe(&path, new_id())
        .map_err(|e| anyhow!("SOUND_UNAVAILABLE: {} is not a sound file Nuzky can play ({e:#})", sound.title))?;
    ensure!(asset.kind == AssetKind::Audio, "SOUND_UNAVAILABLE: {} is not a sound file", sound.title);
    asset.name = sound.title.clone();
    asset.credit = Some(sound.credit());
    Ok(asset)
}

/// The asset of a project that already holds this sound, so adding it again places another clip.
pub fn existing<'a>(assets: &'a [Asset], id: &str) -> Option<&'a Asset> {
    let (source, id) = id.split_once(':')?;
    assets
        .iter()
        .find(|a| a.credit.as_ref().is_some_and(|c| c.source == source && c.id == id) && Path::new(&a.path).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn checksum(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn every_built_in_sound_is_embedded_as_its_manifest_says() {
        let files: Vec<&str> = FILES.iter().map(|(file, _)| *file).collect();
        let listed: Vec<&str> = packed().iter().map(|p| p.file.as_str()).collect();
        assert_eq!(files, listed);
        for (p, (_, bytes)) in packed().iter().zip(FILES) {
            assert_eq!(checksum(bytes), p.sha256, "{} differs from its manifest", p.file);
        }
        assert_eq!(built_in("").len(), FILES.len());
        assert!(
            built_in("WHOOSH").iter().all(|s| s.category.as_deref() == Some("Whoosh") || s.title.contains("hoosh"))
        );
    }

    fn audio(license: &str, version: &str, deed: &str) -> OpenverseAudio {
        serde_json::from_value(json!({
            "id": "b386828e-b628-45f6-b900-6441a49cc977", "title": "Lofy", "creator": "macouno",
            "license": license, "license_version": version, "license_url": deed,
            "foreign_landing_url": "https://www.jamendo.com/track/317391",
            "url": "https://prod-1.storage.jamendo.com/?trackid=317391&format=mp32", "duration": 160000, "source": "jamendo", "filetype": "mp32"
        }))
        .unwrap()
    }

    /// Whatever the catalogue claims, only CC0 1.0 and CC BY with its own deed ever show.
    #[test]
    fn only_cc0_and_cc_by_pass() {
        let ok =
            from_openverse(audio("by", "3.0", "https://creativecommons.org/licenses/by/3.0/"), Kind::Music).unwrap();
        assert_eq!((ok.license, ok.license_version.as_str(), ok.duration_us), (License::CcBy, "3.0", 160_000_000));
        assert_eq!(ok.provider, "Jamendo");
        assert_eq!(file_name(&ok), "openverse-b386828e-b628-45f6-b900-6441a49cc977.mp3");
        assert!(
            from_openverse(audio("cc0", "1.0", "https://creativecommons.org/publicdomain/zero/1.0/"), Kind::Effect)
                .is_some()
        );
        for (license, version, deed) in [
            ("by-nc", "3.0", "https://creativecommons.org/licenses/by-nc/3.0/"),
            ("by-nd", "4.0", "https://creativecommons.org/licenses/by-nd/4.0/"),
            ("by-sa", "4.0", "https://creativecommons.org/licenses/by-sa/4.0/"),
            ("sampling+", "1.0", "https://creativecommons.org/licenses/sampling+/1.0/"),
            ("pdm", "1.0", "https://creativecommons.org/publicdomain/mark/1.0/"),
            // A label that disagrees with the deed it links.
            ("by", "4.0", "https://creativecommons.org/licenses/by-nc/4.0/"),
            ("cc0", "1.0", "https://creativecommons.org/licenses/by-nc/4.0/"),
            ("by", "4.0", "https://example.org/licenses/by/4.0/"),
            ("by", "9.9", "https://creativecommons.org/licenses/by/9.9/"),
            ("by", "3.0", "https://creativecommons.org/licenses/by/4.0/"),
        ] {
            assert!(from_openverse(audio(license, version, deed), Kind::Music).is_none(), "{license} {deed}");
        }
        // CC BY needs someone to credit.
        let mut nobody = audio("by", "4.0", "https://creativecommons.org/licenses/by/4.0/");
        nobody.creator = None;
        assert!(from_openverse(nobody, Kind::Music).is_none());
        // Links the app would open or download must be web addresses.
        let mut local = audio("by", "4.0", "https://creativecommons.org/licenses/by/4.0/");
        local.url = Some("file:///etc/passwd".into());
        assert!(from_openverse(local, Kind::Music).is_none());
    }

    #[test]
    fn freesound_licences_are_read_from_their_deeds() {
        assert_eq!(
            license_of("http://creativecommons.org/licenses/by/4.0/").unwrap().2,
            "https://creativecommons.org/licenses/by/4.0/"
        );
        assert_eq!(license_of("http://creativecommons.org/licenses/by/3.0/us/").unwrap().1, "3.0");
        assert_eq!(license_of("https://creativecommons.org/publicdomain/zero/1.0/").unwrap().0, License::Cc0);
        for deed in [
            "http://creativecommons.org/licenses/by-nc/4.0/",
            "http://creativecommons.org/licenses/sampling+/1.0/",
            "https://creativecommons.org/licenses/by/4.0/../by-nc/4.0/",
        ] {
            assert!(license_of(deed).is_none(), "{deed}");
        }
    }

    /// A server that accepts and then says nothing never holds up a stop, and nothing is left behind.
    #[test]
    fn a_silent_server_never_holds_up_a_stop() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let _held: Vec<_> = listener.incoming().take(1).collect();
            std::thread::sleep(Duration::from_secs(60));
        });
        // Only this module reads the test server's address.
        unsafe { std::env::set_var("NUZKY_OPENVERSE_URL", format!("http://127.0.0.1:{port}/v1/")) };
        let mut silent = built_in("whoosh").remove(0);
        (silent.id, silent.provider, silent.file) =
            ("openverse:silent".into(), "Jamendo".into(), format!("http://127.0.0.1:{port}/song.mp3"));
        remember(&[silent]);
        let cache = std::env::temp_dir().join(format!("nuzky-silent-{}", new_id()));
        let started = std::time::Instant::now();
        let error =
            fetch("openverse:silent", &cache, || started.elapsed() > Duration::from_millis(300), |_| {}).unwrap_err();
        assert!(error.to_string().starts_with("CANCELLED"), "{error:#}");
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        std::thread::sleep(Duration::from_millis(200));
        assert!(std::fs::read_dir(cache.join("sounds")).map_or(true, |mut d| d.next().is_none()));
        std::fs::remove_dir_all(cache).ok();
    }

    #[test]
    fn catalogue_text_becomes_one_short_line() {
        assert_eq!(line(Some("  Rainy\n\tWindow\u{7}  "), 300), "Rainy Window");
        assert_eq!(line(Some(&"x".repeat(400)), 300).len(), 300);
        assert_eq!(query(&[("q", "lo fi & café".into())]), "q=lo%20fi%20%26%20caf%C3%A9");
    }
}
