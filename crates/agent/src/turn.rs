use crate::claude;
use crate::discover::AgentId;
use crate::event::{AgentEvent, ErrorCode};
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// CapOpen's MCP bridge as the agent should start it.
#[derive(Clone, Debug)]
pub struct McpServer {
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Passed explicitly: an agent may give its MCP servers only a few variables of its own.
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub agent: AgentId,
    pub exe: PathBuf,
    /// The conversation: a new one is created under this id, a later turn resumes it.
    pub session: String,
    pub resume: bool,
    pub prompt: String,
    /// An empty folder of CapOpen's own, so no project instructions or settings load from it.
    pub cwd: PathBuf,
    pub mcp: McpServer,
}

/// How long a stopped agent gets to finish before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// One message to the agent, running as its own process group.
pub struct Turn {
    pid: u32,
    stopping: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

/// Variables of a parent Claude Code session (CapOpen started from one, as in development),
/// which would connect the agent to that session instead of starting its own.
fn parent_session_var(name: &str) -> bool {
    name == "CLAUDECODE"
        || name == "CLAUDE_PID"
        || name.starts_with("CLAUDE_CODE_")
        || name.starts_with("CLAUDE_AGENT_SDK")
}

impl Turn {
    /// Starts the turn; `emit` gets its events from a reader thread, ending with `Done` or `Error`.
    pub fn start(req: TurnRequest, mut emit: impl FnMut(AgentEvent) + Send + 'static) -> Result<Turn> {
        if req.agent != AgentId::Claude {
            bail!(
                "{} in the AI panel is not ready yet. Use Claude Code, or connect Codex in a terminal.",
                req.agent.name()
            );
        }
        if cfg!(windows) {
            bail!("The AI panel is not available on Windows yet. Connect your agent in a terminal instead.");
        }
        std::fs::create_dir_all(&req.cwd).context("Creating the AI panel's folder")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&req.cwd, std::fs::Permissions::from_mode(0o700))
                .context("Securing the AI panel's folder")?;
        }
        let mut cmd = claude::command(&req);
        cmd.current_dir(&req.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(parent_session_var) {
                cmd.env_remove(name);
            }
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn().with_context(|| format!("NOT_INSTALLED: could not start {}", req.exe.display()))?;
        let pid = child.id();
        let stopping = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));

        let mut stdin = child.stdin.take().context("Agent input")?;
        let prompt = req.prompt;
        std::thread::spawn(move || {
            // Closing input tells `claude -p` that the message is complete.
            let _ = stdin.write_all(prompt.as_bytes());
        });
        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let stderr = child.stderr.take().context("Agent errors")?;
        let errors = tail.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(std::result::Result::ok) {
                let mut tail = errors.lock().unwrap();
                if tail.len() == 8 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        let stdout = child.stdout.take().context("Agent output")?;
        let (stop_flag, done_flag) = (stopping.clone(), finished.clone());
        std::thread::spawn(move || {
            let mut parser = claude::Parser::default();
            let mut failed: Option<(ErrorCode, String)> = None;
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                // Lines can be long (whole tool results); read_line keeps them whole.
                match reader.by_ref().read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                for step in parser.line(line.trim_end()) {
                    match step {
                        claude::Step::Event(e) if failed.is_none() => emit(e),
                        claude::Step::Fail(code, message) if failed.is_none() => {
                            failed = Some((code, message));
                            signal(pid, false);
                        }
                        claude::Step::ResultError(message)
                            if failed.is_none() && !stop_flag.load(Ordering::Acquire) =>
                        {
                            failed = Some((ErrorCode::AgentFailed, message));
                        }
                        _ => {}
                    }
                }
            }
            let status = child.wait();
            done_flag.store(true, Ordering::Release);
            let end = if let Some((code, message)) = failed {
                AgentEvent::Error { code, message }
            } else if stop_flag.load(Ordering::Acquire) {
                AgentEvent::Done { stopped: true }
            } else if status.as_ref().is_ok_and(|s| s.success()) {
                AgentEvent::Done { stopped: false }
            } else {
                let tail = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                let code = status.ok().and_then(|s| s.code()).map_or("a signal".to_owned(), |c| format!("code {c}"));
                AgentEvent::Error {
                    code: ErrorCode::AgentFailed,
                    message: if tail.trim().is_empty() { format!("The agent exited with {code}.") } else { tail },
                }
            };
            emit(end);
        });
        Ok(Turn { pid, stopping, finished })
    }

    /// Asks the agent to stop, and kills it and its MCP bridge if it has not after a short grace.
    pub fn stop(&self) {
        if self.finished.load(Ordering::Acquire) || self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        signal(self.pid, false);
        let (pid, finished) = (self.pid, self.finished.clone());
        std::thread::spawn(move || {
            std::thread::sleep(STOP_GRACE);
            if !finished.load(Ordering::Acquire) {
                signal(pid, true);
            }
        });
    }

    pub fn running(&self) -> bool {
        !self.finished.load(Ordering::Acquire)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.stop();
    }
}

/// SIGINT lets Claude record the interrupted turn so the next one can resume; SIGKILL is the fallback.
/// The whole process group gets it, so the MCP bridge the agent started goes too.
fn signal(pid: u32, kill: bool) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), if kill { libc::SIGKILL } else { libc::SIGINT });
    }
    #[cfg(not(unix))]
    let _ = (pid, kill);
}
