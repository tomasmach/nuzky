use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentId {
    Claude,
    Codex,
}

impl AgentId {
    pub fn name(self) -> &'static str {
        match self {
            AgentId::Claude => "Claude Code",
            AgentId::Codex => "Codex",
        }
    }

    fn program(self) -> &'static str {
        match self {
            AgentId::Claude => "claude",
            AgentId::Codex => "codex",
        }
    }

    /// Points the panel at another executable, such as the fake agent of the E2E tests.
    fn override_var(self) -> &'static str {
        match self {
            AgentId::Claude => "NUZKY_AGENT_CLAUDE",
            AgentId::Codex => "NUZKY_AGENT_CODEX",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: AgentId,
    pub name: &'static str,
    /// The executable, or None when it is not installed.
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

/// Where installers put these CLIs when the app was started without the user's shell `PATH`,
/// e.g. from a desktop launcher.
fn known_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        for d in [".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin"] {
            dirs.push(home.join(d));
        }
    }
    dirs.extend(["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"].map(PathBuf::from));
    dirs
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn locate(id: AgentId) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(id.override_var()) {
        return Some(PathBuf::from(path)).filter(|p| executable(p));
    }
    let name = if cfg!(windows) { format!("{}.exe", id.program()) } else { id.program().to_owned() };
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    path.into_iter().chain(known_dirs(home.as_deref())).map(|dir| dir.join(&name)).find(|p| executable(p))
}

/// The first version-looking word of `--version`: "2.1.292 (Claude Code)", "codex-cli 0.160.1".
fn version_of(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(str::to_owned)
}

fn version(path: &Path) -> Option<String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
    version_of(&out)
}

/// Finds the installed agent and its version; a missing one has no path.
pub fn find(id: AgentId) -> AgentInfo {
    let path = locate(id);
    let version = path.as_deref().and_then(version);
    AgentInfo { id, name: id.name(), path, version }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_from_both_agents_wording() {
        assert_eq!(version_of("2.1.292 (Claude Code)\n").as_deref(), Some("2.1.292"));
        assert_eq!(version_of("codex-cli 0.160.1\n").as_deref(), Some("0.160.1"));
        assert_eq!(version_of("unknown"), None);
    }
}
