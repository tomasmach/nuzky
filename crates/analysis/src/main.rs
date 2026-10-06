use std::{io::Write, path::PathBuf};

use anyhow::{Context, Result, bail};
use capopen_analysis::{
    AudioSource, SceneParams, SilenceParams, filler_words, integrated_lufs, loudness, scene_cuts,
    silences, transcribe_words,
};

const USAGE: &str = "capopen-analyze <media> <loudness|silences|scenes|words|fillers> [--lang auto|cs|en] [--model path] [--vad-model path] [--cache path]";

enum Command {
    Loudness,
    Silences,
    Scenes,
    Words,
    Fillers,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let media = PathBuf::from(args.next().context(USAGE)?);
    let command = match args.next().as_deref() {
        Some("loudness") => Command::Loudness,
        Some("silences") => Command::Silences,
        Some("scenes") => Command::Scenes,
        Some("words") => Command::Words,
        Some("fillers") => Command::Fillers,
        _ => bail!(USAGE),
    };
    let mut language = "auto".to_owned();
    let mut model = None;
    let mut vad = None;
    let mut cache = None;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("Missing value for {arg}"))?;
        match arg.as_str() {
            "--lang" => language = value,
            "--model" => model = Some(PathBuf::from(value)),
            "--vad-model" => vad = Some(PathBuf::from(value)),
            "--cache" => cache = Some(PathBuf::from(value)),
            _ => bail!("Unknown argument: {arg}\n{USAGE}"),
        }
    }
    let asset = capopen_engine::media::probe(&media, cache_id(&media)?).context("Probing media")?;
    let cache = cache.unwrap_or_else(|| std::env::temp_dir().join("capopen-analysis"));
    let output = match command {
        Command::Loudness => serde_json::json!({
            "window_us": 100_000, "rms_dbfs": loudness(&asset, &cache, 100_000)?,
            "integrated_lufs_approx": integrated_lufs(&asset, &cache)?,
        }),
        Command::Silences => {
            serde_json::to_value(silences(&asset, &cache, SilenceParams::default())?)?
        }
        Command::Scenes => serde_json::to_value(scene_cuts(&asset, SceneParams::default())?)?,
        Command::Words | Command::Fillers => {
            let model = model
                .or_else(|| find_model("ggml-small.bin"))
                .context("Pass --model with a local Whisper model path")?;
            let vad = vad
                .or_else(|| {
                    model
                        .parent()
                        .map(|p| p.join("ggml-silero-v5.1.2.bin"))
                        .filter(|p| p.is_file())
                })
                .or_else(|| find_model("ggml-silero-v5.1.2.bin"))
                .context("Pass --vad-model with a local Silero model path")?;
            let transcript = transcribe_words(
                AudioSource::Asset {
                    asset: &asset,
                    cache: &cache,
                },
                &model,
                &vad,
                &language,
            )?;
            match command {
                Command::Words => serde_json::to_value(transcript)?,
                _ => serde_json::to_value(filler_words(&transcript, &language))?,
            }
        }
    };
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &output).context("Writing JSON")?;
    writeln!(stdout).context("Writing JSON newline")?;
    Ok(())
}

fn find_model(name: &str) -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    data.map(|p| p.join("capopen/models").join(name))
        .filter(|p| p.is_file())
        .or_else(|| {
            let p = PathBuf::from("tmp-test/xdg/data/capopen/models").join(name);
            p.is_file().then_some(p)
        })
}

fn cache_id(path: &std::path::Path) -> Result<String> {
    use std::hash::{Hash, Hasher};
    let path = std::fs::canonicalize(path).context("Resolving media path")?;
    let metadata = path.metadata().context("Reading media metadata")?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    metadata.len().hash(&mut hash);
    metadata
        .modified()
        .context("Reading media modification time")?
        .hash(&mut hash);
    Ok(format!("analysis-{:016x}", hash.finish()))
}
