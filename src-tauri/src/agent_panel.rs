//! The AI panel: runs the user's own agent for one message at a time, with Nuzky's MCP bridge
//! as its only tools, and streams what it does to the UI as `agent` events.

use crate::{AppState, CmdResult, connect, err};
use nuzky_agent::{AgentEvent, AgentId, AgentInfo, McpServer, Turn, TurnRequest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};

/// One conversation with an agent; a later message resumes its session.
struct Chat {
    agent: AgentId,
    session: String,
    /// Whether a turn got far enough for the agent to keep the conversation.
    resumable: bool,
}

#[derive(Default)]
pub struct AgentPanel {
    chats: Mutex<HashMap<String, Chat>>,
    /// The message the agent is working on, by the id the UI gave it.
    active: Mutex<Option<(String, Turn)>>,
    /// A message the user stopped while it was still starting; written under the `active` lock.
    cancelled: Mutex<Option<String>>,
    /// Counts stops, so a message still starting when one comes is stopped as soon as it runs.
    stops: AtomicU64,
}

impl AgentPanel {
    /// Stops the panel's agent, e.g. for Stop and edit in the top bar or when another project opens,
    /// also one whose message is still starting.
    pub fn stop(&self) {
        self.stops.fetch_add(1, Ordering::AcqRel);
        if let Some((_, turn)) = self.active.lock().unwrap().as_ref() {
            turn.stop();
        }
    }
}

#[derive(Serialize, Clone)]
struct PanelEvent<'a> {
    chat: &'a str,
    #[serde(flatten)]
    event: &'a AgentEvent,
}

/// What went with the message, frozen when it was sent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptContext {
    selection: Option<Vec<String>>,
    playhead_us: Option<i64>,
    #[serde(default)]
    frame: bool,
    /// Sent from the Your style page: the message is about the creator's style.
    #[serde(default)]
    style: bool,
    /// Make a thumbnail in the AI panel: the agent gets the MCP prompt `thumbnail` instead of the label.
    #[serde(default)]
    thumbnail: bool,
}

fn timecode(us: i64) -> String {
    let s = us.max(0) as f64 / 1e6;
    format!("{:02}:{:05.2}", (s / 60.0).floor() as i64, s % 60.0)
}

/// The message as the agent gets it: the user's words, then what they had selected.
fn prompt(text: &str, context: &PromptContext, project: &nuzky_engine::Project) -> String {
    let thumbnail;
    let text = if context.thumbnail {
        // What the user typed beside Make a thumbnail are their wishes.
        thumbnail = nuzky_mcp::thumbnail_prompt(if text.trim().is_empty() { "none" } else { text });
        thumbnail.as_str()
    } else {
        text
    };
    let mut lines = Vec::new();
    if context.style {
        lines.push(
            "[Nuzky] The user writes from the Your style page: this is about how you edit all their videos, not the open project. Read their style with get_style. Agree on the exact change with them first, then make it with change_style (their words as setOwn) and say in a sentence what changed; the page shows it at once.".to_owned(),
        );
    }
    if let Some(ids) = context.selection.as_ref().filter(|ids| !ids.is_empty()) {
        let clips: Vec<_> = project.tracks.iter().flat_map(|t| &t.clips).filter(|c| ids.contains(&c.id)).collect();
        let range = clips.iter().map(|c| c.start_us).min().zip(clips.iter().map(|c| c.end_us()).max());
        let range =
            range.map_or(String::new(), |(a, b)| format!(" covering {}–{} ({a}–{b} µs)", timecode(a), timecode(b)));
        lines.push(format!("[Nuzky] Selected clips: {}{range}.", ids.join(", ")));
    }
    if let Some(at) = context.playhead_us {
        lines.push(format!("[Nuzky] Playhead: {} ({at} µs).", timecode(at)));
        if context.frame {
            lines.push(format!(
                "[Nuzky] The user points at the frame at the playhead: look at it with inspect_frames at {at} µs."
            ));
        }
    }
    if context.thumbnail {
        // A frame the user chose stays: the prompt would otherwise pick one anew.
        for cover in &project.thumbnails {
            let name = match cover.format {
                nuzky_engine::model::ThumbnailFormat::Cover9x16 => "cover_9x16",
                nuzky_engine::model::ThumbnailFormat::Youtube16x9 => "youtube_16x9",
            };
            lines.push(format!(
                "[Nuzky] The project's {name} already uses the frame at {} ({} µs): keep that frame unless the user's wishes say otherwise.",
                timecode(cover.time_us),
                cover.time_us
            ));
        }
    }
    if lines.is_empty() { text.to_owned() } else { format!("{text}\n\n{}", lines.join("\n")) }
}

/// An empty folder of Nuzky's own outside the home folder, so no CLAUDE.md, AGENTS.md or
/// project settings above it load into the agent.
fn work_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("nuzky-agent")
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tauri::command]
pub async fn agent_list() -> CmdResult<Vec<AgentInfo>> {
    tauri::async_runtime::spawn_blocking(|| [AgentId::Claude, AgentId::Codex].map(nuzky_agent::find).to_vec())
        .await
        .map_err(err)
}

/// Sends a message in the chat the UI named, under the id it gave the message; the answer arrives
/// as `agent` events for the chat. The UI names both before sending, so even an agent that fails
/// at once reaches it, and Stop works while the message is still starting.
#[tauri::command]
pub async fn agent_send(
    app: AppHandle,
    agent: AgentId,
    chat: String,
    turn: String,
    text: String,
    context: PromptContext,
) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || send(&app, agent, chat, turn, &text, &context)).await.map_err(err)?
}

fn send(
    app: &AppHandle,
    agent: AgentId,
    chat: String,
    turn: String,
    text: &str,
    context: &PromptContext,
) -> CmdResult<()> {
    let panel = app.state::<AgentPanel>();
    // Another project opening, or Stop, while this starts must stop it once it runs.
    let stops = panel.stops.load(Ordering::Acquire);
    // The panel's lock and the session's are never held together, so Stop and edit cannot deadlock with this.
    let busy = || panel.active.lock().unwrap().as_ref().is_some_and(|(_, turn)| turn.running());
    if busy() {
        return Err("AGENT_BUSY: The agent is still answering. Wait for it, or stop it.".into());
    }
    let state = app.state::<AppState>();
    let (project, path) = {
        let current = state.session.lock().unwrap();
        let session = current.host.session.state().map_err(err)?;
        if let Some(run) = session.open_run {
            return Err(format!(
                "AGENT_BUSY: Another agent is editing ({}). Wait for it, or stop it in the top bar.",
                run.label
            ));
        }
        (session.project, current.path.clone())
    };
    let info = nuzky_agent::find(agent);
    let exe = info.path.ok_or_else(|| format!("NOT_INSTALLED: {} is not installed.", agent.name()))?;
    // The bridge attaches to this window's project by its path: `--current` names whatever project
    // the window opened last made current, which may be another Nuzky window's.
    let bridge = connect::Command::this_app().map_err(err)?;
    let args = vec!["mcp".into(), "--project".into(), path.to_string_lossy().into_owned(), "--allow-write".into()];
    let env = std::env::var("XDG_RUNTIME_DIR").map(|dir| vec![("XDG_RUNTIME_DIR".to_owned(), dir)]).unwrap_or_default();

    let mut chats = panel.chats.lock().unwrap();
    if chats.get(&chat).is_some_and(|c| c.agent != agent) {
        chats.remove(&chat);
    }
    let id = chat;
    let chat = chats.entry(id.clone()).or_insert_with(|| Chat { agent, session: new_id(), resumable: false });
    let request = TurnRequest {
        agent,
        exe,
        session: chat.session.clone(),
        resume: chat.resumable,
        prompt: prompt(text, context, &project),
        cwd: work_dir(),
        mcp: McpServer { command: bridge.program.into(), args, env },
    };
    drop(chats);
    if panel.cancelled.lock().unwrap().as_deref() == Some(turn.as_str()) {
        app.emit("agent", PanelEvent { chat: &id, event: &AgentEvent::Done { stopped: true } }).ok();
        return Ok(());
    }
    let (handle, chat_id) = (app.clone(), id.clone());
    let started = Turn::start(request, move |event| {
        handle.emit("agent", PanelEvent { chat: &chat_id, event: &event }).ok();
        let ended = matches!(event, AgentEvent::Done { .. } | AgentEvent::Error { .. });
        if !ended {
            return;
        }
        let panel = handle.state::<AgentPanel>();
        if let Some(chat) = panel.chats.lock().unwrap().get_mut(&chat_id) {
            // A turn that never got going leaves no conversation to resume, so the next one starts it afresh.
            match event {
                AgentEvent::Done { .. } => chat.resumable = true,
                _ if !chat.resumable => chat.session = new_id(),
                _ => {}
            }
        }
    })
    .map_err(err)?;
    let mut active = panel.active.lock().unwrap();
    if panel.stops.load(Ordering::Acquire) != stops || panel.cancelled.lock().unwrap().as_deref() == Some(turn.as_str())
    {
        started.stop();
    }
    *active = Some((turn, started));
    Ok(())
}

/// Stops the agent: Nuzky ends the run first, so the editor unlocks at once whatever the agent does.
/// A message still starting is stopped as soon as it runs.
#[tauri::command]
pub fn agent_stop(state: State<'_, AppState>, panel: State<'_, AgentPanel>, turn: String) -> CmdResult<()> {
    {
        let active = panel.active.lock().unwrap();
        if !active.as_ref().is_some_and(|(id, started)| *id == turn && started.running()) {
            *panel.cancelled.lock().unwrap() = Some(turn);
            return Ok(());
        }
    }
    let ended = state.session.lock().unwrap().host.stop_run();
    // The agent stops even when ending the run failed, e.g. on a full disk.
    panel.stop();
    match ended {
        // The agent may end its run itself at this very moment; then there was nothing left to stop.
        Err(error) if !error.to_string().starts_with("INVALID_RUN") => Err(err(error)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::Project;

    #[test]
    fn the_prompt_carries_what_was_selected_when_sent() {
        let project = Project::new("t");
        let context = PromptContext {
            selection: None,
            playhead_us: Some(7_200_000),
            frame: true,
            style: false,
            thumbnail: false,
        };
        let text = prompt("Zkrať to", &context, &project);
        assert!(text.starts_with("Zkrať to\n\n[Nuzky] Playhead: 00:07.20 (7200000 µs)."), "{text}");
        assert!(text.contains("inspect_frames at 7200000 µs"));
        let none = PromptContext {
            selection: Some(Vec::new()),
            playhead_us: None,
            frame: true,
            style: false,
            thumbnail: false,
        };
        assert_eq!(prompt("Ahoj", &none, &project), "Ahoj");
        let style = PromptContext { selection: None, playhead_us: None, frame: false, style: true, thumbnail: false };
        let text = prompt("Nestříhej mi rozloučení", &style, &project);
        assert!(
            text.starts_with("Nestříhej mi rozloučení\n\n[Nuzky] The user writes from the Your style page")
                && text.contains("change_style"),
            "{text}"
        );
        let thumbnail =
            PromptContext { selection: None, playhead_us: Some(0), frame: false, style: false, thumbnail: true };
        let text = prompt("", &thumbnail, &project);
        assert!(text.starts_with("Make a 9:16 cover") && text.contains("export_thumbnail"), "{text}");
        assert!(
            text.contains("wishes, which win over the steps: none") && text.contains("[Nuzky] Playhead: 00:00.00"),
            "{text}"
        );
        let text = prompt("Hook: I QUIT SUGAR", &thumbnail, &project);
        assert!(text.contains("wishes, which win over the steps: Hook: I QUIT SUGAR"), "{text}");
        let mut covered = Project::new("t");
        covered
            .thumbnails
            .push(serde_json::from_value(serde_json::json!({"format": "cover_9x16", "timeUs": 10_620_000})).unwrap());
        let text = prompt("", &thumbnail, &covered);
        assert!(
            text.contains("[Nuzky] The project's cover_9x16 already uses the frame at 00:10.62 (10620000 µs)"),
            "{text}"
        );
    }
}
