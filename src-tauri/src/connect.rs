//! Connect your agent: writes the `nuzky` MCP server into Claude Code's and Codex's own
//! configuration, after a backup, and changes nothing else in those files. The entry runs this
//! app's `mcp --current`, so the agent edits whichever project is open here, live.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

pub const NAME: &str = "nuzky";

#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "AgentKind"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::ClaudeCode, Agent::Codex];

    fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// Where the agent keeps its user-wide MCP servers, honouring its own override variable.
    pub fn config(self, home: &Path) -> PathBuf {
        let from = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
        match self {
            Agent::ClaudeCode => from("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.to_path_buf()).join(".claude.json"),
            Agent::Codex => from("CODEX_HOME").unwrap_or_else(|| home.join(".codex")).join("config.toml"),
        }
    }
}

/// What the agent runs to reach Nuzky.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Command {
    /// This app's own executable; an AppImage runs from a temporary mount, so the AppImage itself.
    pub fn this_app() -> Result<Self> {
        let exe = std::env::current_exe().context("Finding the Nuzky executable")?;
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let program = app_image(&exe, var("APPDIR"), var("APPIMAGE")).unwrap_or(exe);
        Ok(Self {
            program: program.to_string_lossy().into_owned(),
            args: ["mcp", "--current", "--allow-write"].map(String::from).to_vec(),
        })
    }
}

/// The AppImage this executable runs from. Children inherit APPIMAGE, so started from another
/// AppImage's terminal it names that one; only an executable inside APPDIR is its own.
fn app_image(exe: &Path, dir: Option<PathBuf>, image: Option<PathBuf>) -> Option<PathBuf> {
    image.filter(|_| dir.is_some_and(|dir| exe.starts_with(dir)))
}

#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// The entry runs this app.
    Connected,
    /// A `nuzky` entry runs something else, such as another copy of Nuzky.
    Other,
    /// No `nuzky` entry.
    Missing,
    /// The file is not valid; connecting would not touch it.
    Unreadable,
}

/// An agent Nuzky can be connected to, and the state of its `nuzky` MCP entry.
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "AgentConnection"))]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub agent: Agent,
    pub name: &'static str,
    /// Its config file.
    pub path: String,
    /// `connected`: the entry runs this app. `other`: a `nuzky` entry runs something else. `missing`: no
    /// `nuzky` entry. `unreadable`: the file is not valid, and connecting would not touch it.
    #[cfg_attr(test, ts(inline))]
    pub state: State,
    /// Why the file could not be read.
    pub problem: Option<String>,
    /// The copy made before the last change.
    pub backup: Option<String>,
}

impl Connection {
    /// Paths in the home folder as `~/…`, as people read them.
    pub fn shown_from(mut self, home: &Path) -> Self {
        self.path = tilde(&self.path, home);
        self.backup = self.backup.map(|backup| tilde(&backup, home));
        self
    }
}

fn tilde(path: &str, home: &Path) -> String {
    match Path::new(path).strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

pub fn status(agent: Agent, path: &Path, command: &Command) -> Connection {
    let (state, problem) = match read(path).and_then(|text| entry(agent, text.as_deref())) {
        Ok(None) => (State::Missing, None),
        Ok(Some(found)) if &found == command => (State::Connected, None),
        Ok(Some(_)) => (State::Other, None),
        Err(error) => (State::Unreadable, Some(format!("{error:#}"))),
    };
    Connection { agent, name: agent.name(), path: path.display().to_string(), state, problem, backup: None }
}

/// Writes the entry unless it is already there. The file is copied first, next to itself, and
/// replaced in one step; a symlinked config is written where the link points.
pub fn connect(agent: Agent, path: &Path, command: &Command, stamp: u64) -> Result<Connection> {
    let path = resolved(path)?;
    let before = read(&path)?;
    let mut connection = status(agent, &path, command);
    match connection.state {
        State::Connected => return Ok(connection),
        State::Unreadable => bail!(
            "{} is not valid, so Nuzky left it alone: {}",
            path.display(),
            connection.problem.as_deref().unwrap_or_default()
        ),
        State::Missing | State::Other => {}
    }
    let text = match agent {
        Agent::ClaudeCode => with_claude_entry(before.as_deref(), command)?,
        Agent::Codex => with_codex_entry(before.as_deref(), command)?,
    };
    let backup = match &before {
        Some(_) => {
            let backup = path.with_file_name(format!(
                "{}.nuzky-backup-{stamp}",
                path.file_name().context("Config has no file name")?.to_string_lossy()
            ));
            std::fs::copy(&path, &backup).with_context(|| format!("Backing up {}", path.display()))?;
            Some(backup)
        }
        None => None,
    };
    replace(&path, text.as_bytes())?;
    connection = status(agent, &path, command);
    ensure!(connection.state == State::Connected, "{} did not take the Nuzky entry", path.display());
    connection.backup = backup.map(|b| b.display().to_string());
    Ok(connection)
}

fn resolved(path: &Path) -> Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(path).with_context(|| format!("Following the link {}", path.display()))
        }
        _ => Ok(path.to_path_buf()),
    }
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Reading {}", path.display())),
    }
}

/// The new file next to the old one, then renamed over it, with the old file's permissions; a new
/// file is private, since agent configs can hold tokens.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().context("Config has no folder")?;
    std::fs::create_dir_all(directory).with_context(|| format!("Creating {}", directory.display()))?;
    let part = directory.join(format!(
        ".{}.nuzky-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).map_or(0o600, |m| m.permissions().mode() & 0o7777);
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&part, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.with_context(|| format!("Writing {}", path.display()))
}

fn entry(agent: Agent, text: Option<&str>) -> Result<Option<Command>> {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return Ok(None) };
    let found = match agent {
        Agent::ClaudeCode => {
            let config: Value = serde_json::from_str(text).context("Not valid JSON")?;
            ensure!(config.is_object(), "Not a JSON object");
            config
                .get("mcpServers")
                .and_then(|s| s.get(NAME))
                .map(|e| (e.get("command").cloned(), e.get("args").cloned()))
        }
        Agent::Codex => {
            let config: toml_edit::DocumentMut = text.parse().context("Not valid TOML")?;
            config.get("mcp_servers").and_then(|s| s.get(NAME)).map(|e| {
                let value = |key| e.get(key).and_then(|i| i.as_value()).map(toml_to_json);
                // Codex does not start a server turned off, so that entry runs nothing.
                match e.get("enabled").and_then(|i| i.as_bool()) {
                    Some(false) => (None, None),
                    _ => (value("command"), value("args")),
                }
            })
        }
    };
    Ok(found.map(|(program, args)| Command {
        program: program.and_then(|p| p.as_str().map(String::from)).unwrap_or_default(),
        args: args
            .and_then(|a| a.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()))
            .unwrap_or_default(),
    }))
}

fn toml_to_json(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        _ => Value::Null,
    }
}

/// Claude Code keeps user-wide servers under `mcpServers` in `.claude.json`, a file it also
/// writes itself. Every other key keeps its exact text: only that object is rebuilt.
fn with_claude_entry(text: Option<&str>, command: &Command) -> Result<String> {
    let server = serde_json::json!({"type": "stdio", "command": command.program, "args": command.args, "env": {}});
    let mut top = match text.filter(|t| !t.trim().is_empty()) {
        Some(text) => serde_json::from_str::<Entries>(text).context("Not valid JSON")?.0,
        None => Vec::new(),
    };
    let servers = top.iter().position(|(key, _)| key == "mcpServers");
    let mut inner = match servers {
        Some(i) => serde_json::from_str::<Entries>(top[i].1.get()).context("mcpServers is not an object")?.0,
        None => Vec::new(),
    };
    let ours = RawValue::from_string(indent(&serde_json::to_string_pretty(&server)?, "    "))?;
    match inner.iter().position(|(key, _)| key == NAME) {
        Some(i) => inner[i].1 = ours,
        None => inner.push((NAME.into(), ours)),
    }
    let rebuilt = RawValue::from_string(object(&inner, "  ")?)?;
    match servers {
        Some(i) => top[i].1 = rebuilt,
        None => top.push(("mcpServers".into(), rebuilt)),
    }
    Ok(object(&top, "")? + "\n")
}

fn indent(text: &str, by: &str) -> String {
    text.replace('\n', &format!("\n{by}"))
}

fn object(entries: &[(String, Box<RawValue>)], outer: &str) -> Result<String> {
    if entries.is_empty() {
        return Ok("{}".into());
    }
    let lines = entries
        .iter()
        .map(|(key, value)| Ok(format!("{outer}  {}: {}", serde_json::to_string(key)?, value.get())))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("{{\n{}\n{outer}}}", lines.join(",\n")))
}

/// A JSON object's members in file order, each value as its original text.
struct Entries(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Entries;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> std::result::Result<Entries, M::Error> {
                let mut entries: Vec<(String, Box<RawValue>)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    // JSON parsers keep the last of repeated keys; so does this.
                    entries.retain(|(k, _)| *k != key);
                    entries.push((key, value));
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

/// Codex keeps servers as `[mcp_servers.<name>]` tables in `config.toml`; editing the document
/// keeps every comment and the formatting of the rest.
fn with_codex_entry(text: Option<&str>, command: &Command) -> Result<String> {
    let mut config: toml_edit::DocumentMut = text.unwrap_or_default().parse().context("Not valid TOML")?;
    let servers = config.entry("mcp_servers").or_insert_with(|| {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    let servers = servers.as_table_like_mut().context("mcp_servers is not a table")?;
    let mut ours = toml_edit::Table::new();
    ours.insert("command", toml_edit::value(command.program.as_str()));
    ours.insert("args", toml_edit::value(command.args.iter().map(String::as_str).collect::<toml_edit::Array>()));
    servers.insert(NAME, toml_edit::Item::Table(ours));
    Ok(config.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Command {
        Command {
            program: "/opt/Nuzky/nuzky-app".into(),
            args: vec!["mcp".into(), "--current".into(), "--allow-write".into()],
        }
    }

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nuzky-connect-{}", nuzky_engine::edit::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CLAUDE: &str = "{\n  \"numStartups\": 42,\n  \"projects\": {\n    \"/home/me\": {\n      \"allowedTools\": []\n    }\n  },\n  \"mcpServers\": {\n    \"other\": {\n      \"type\": \"stdio\",\n      \"command\": \"other-server\"\n    }\n  },\n  \"theme\": \"dark\"\n}\n";

    #[test]
    fn claude_code_gets_only_the_nuzky_entry_and_keeps_every_other_byte() {
        let dir = dir();
        let path = dir.join(".claude.json");
        std::fs::write(&path, CLAUDE).unwrap();
        assert_eq!(status(Agent::ClaudeCode, &path, &command()).state, State::Missing);
        let done = connect(Agent::ClaudeCode, &path, &command(), 7).unwrap();
        assert_eq!(done.state, State::Connected);
        assert_eq!(std::fs::read_to_string(dir.join(".claude.json.nuzky-backup-7")).unwrap(), CLAUDE);
        let text = std::fs::read_to_string(&path).unwrap();
        // Everything before the new entry is the same text, and so is everything after it.
        let (head, tail) = CLAUDE.split_once("\n  },\n  \"theme\"").unwrap();
        assert!(text.starts_with(head) && text.ends_with(&format!("\n  }},\n  \"theme\"{tail}")), "{text}");
        let config: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(config["mcpServers"]["other"]["command"], "other-server");
        assert_eq!(config["mcpServers"][NAME]["args"], serde_json::json!(["mcp", "--current", "--allow-write"]));
        assert_eq!(config["numStartups"], 42);
        // Connecting again changes nothing and makes no backup.
        let again = connect(Agent::ClaudeCode, &path, &command(), 8).unwrap();
        assert!(again.backup.is_none() && !dir.join(".claude.json.nuzky-backup-8").exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        std::fs::remove_dir_all(dir).unwrap();
    }

    const CODEX: &str = "# my settings\nmodel = \"gpt-6.1-sol\" # the best\n\n[mcp_servers.other]\ncommand = \"other\"\nargs = [\"--x\"]\n\n[profiles.fast]\nmodel_reasoning_effort = \"low\"\n";

    #[test]
    fn codex_gets_only_the_nuzky_table_and_keeps_comments() {
        let dir = dir();
        let path = dir.join("config.toml");
        std::fs::write(&path, CODEX).unwrap();
        let done = connect(Agent::Codex, &path, &command(), 3).unwrap();
        assert_eq!((done.state, done.backup.is_some()), (State::Connected, true));
        let text = std::fs::read_to_string(&path).unwrap();
        // Without the new table the file is exactly what it was, comments included.
        let ours = "[mcp_servers.nuzky]\ncommand = \"/opt/Nuzky/nuzky-app\"\nargs = [\"mcp\", \"--current\", \"--allow-write\"]\n\n";
        assert!(text.contains(ours), "{text}");
        assert_eq!(text.replace(ours, ""), CODEX);
        // Another Nuzky entry is replaced, after a new backup.
        let moved = Command { program: "/new/nuzky-app".into(), ..command() };
        assert_eq!(status(Agent::Codex, &path, &moved).state, State::Other);
        connect(Agent::Codex, &path, &moved, 4).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[mcp_servers.nuzky]").count(), 1);
        assert!(text.contains("/new/nuzky-app") && !text.contains("/opt/Nuzky"));
        // An entry turned off in Codex runs nothing, so it is turned back on by connecting again.
        std::fs::write(&path, text.replace("[mcp_servers.nuzky]\n", "[mcp_servers.nuzky]\nenabled = false\n")).unwrap();
        assert_eq!(status(Agent::Codex, &path, &moved).state, State::Other);
        assert_eq!(connect(Agent::Codex, &path, &moved, 5).unwrap().state, State::Connected);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("enabled"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn missing_files_are_created_private_and_broken_ones_are_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir();
        let codex = dir.join("codex/config.toml");
        connect(Agent::Codex, &codex, &command(), 1).unwrap();
        assert_eq!(std::fs::metadata(&codex).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_to_string(&codex).unwrap().starts_with("[mcp_servers.nuzky]"));
        let claude = dir.join(".claude.json");
        connect(Agent::ClaudeCode, &claude, &command(), 1).unwrap();
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
        assert_eq!(config["mcpServers"][NAME]["command"], "/opt/Nuzky/nuzky-app");
        std::fs::write(&claude, "{ not json").unwrap();
        assert_eq!(status(Agent::ClaudeCode, &claude, &command()).state, State::Unreadable);
        assert!(connect(Agent::ClaudeCode, &claude, &command(), 2).is_err());
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), "{ not json");
        assert!(!dir.join(".claude.json.nuzky-backup-2").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_an_executable_inside_its_appimage_names_the_appimage() {
        let image = Some(PathBuf::from("/home/me/Nuzky.AppImage"));
        let mount = Some(PathBuf::from("/tmp/.mount_Nuzky.Ab12Cd"));
        let inside = Path::new("/tmp/.mount_Nuzky.Ab12Cd/usr/bin/nuzky-app");
        assert_eq!(app_image(inside, mount.clone(), image.clone()), image);
        // A dev build started from another AppImage's terminal inherits its variables.
        let other = Some(PathBuf::from("/tmp/.mount_T3-Cod"));
        assert_eq!(app_image(Path::new("/src/target/debug/nuzky-app"), other, image.clone()), None);
        assert_eq!(app_image(inside, None, image), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_written_where_the_link_points() {
        let dir = dir();
        std::fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let real = dir.join("dotfiles/claude.json");
        std::fs::write(&real, CLAUDE).unwrap();
        let link = dir.join(".claude.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        connect(Agent::ClaudeCode, &link, &command(), 5).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(status(Agent::ClaudeCode, &link, &command()).state, State::Connected);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
